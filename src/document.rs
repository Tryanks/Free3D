//! Editable CAD document, selection state, and snapshot-based undo history.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use occt::Shape;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use glam::{DVec2, DVec3, Mat4};
use serde::{Deserialize, Serialize};

use crate::assembly::{Joint, JointId};
use crate::constraint::{Constraint, solve_items};
use crate::drawing::Drawing;
use crate::history::{
    HistoryOp, HistoryStep, HoleCut, HoleKind, PathRef, PrimitiveKind, ReplayError, edge_ref,
    face_ref,
};
use crate::sketch::{Sketch, SketchEntity, SketchId, SketchItem, SketchPlane};
use crate::tools::extrude::{ExtrudeDrag, ExtrudeMode, ProfileExtrudeDrag};
use crate::ui::expr;

const MAX_SNAPSHOTS: usize = 64;

/// The legacy neutral body colour used when a project has no material data.
pub const DEFAULT_BODY_COLOR: [f32; 3] = [0.66, 0.67, 0.69];

/// Cheap viewport material parameters persisted with every body.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Material {
    /// Surface colour in sRGB component space.
    pub base_color: [f32; 3],
    /// Metallic response in the inclusive 0..=1 range.
    pub metallic: f32,
    /// Microsurface roughness in the inclusive 0..=1 range.
    pub roughness: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            base_color: DEFAULT_BODY_COLOR,
            metallic: 0.0,
            roughness: 0.55,
        }
    }
}

impl Material {
    /// Returns a sanitized copy suitable for storage and GPU upload.
    pub fn clamped(self) -> Self {
        Self {
            base_color: self.base_color.map(|value| value.clamp(0.0, 1.0)),
            metallic: self.metallic.clamp(0.0, 1.0),
            roughness: self.roughness.clamp(0.04, 1.0),
        }
    }
}

/// Converts hue (degrees), saturation and lightness (percent) to sRGB.
pub fn hsl_to_rgb(hue: f32, saturation: f32, lightness: f32) -> [f32; 3] {
    let h = hue.rem_euclid(360.0) / 360.0;
    let s = (saturation / 100.0).clamp(0.0, 1.0);
    let l = (lightness / 100.0).clamp(0.0, 1.0);
    let a = s * l.min(1.0 - l);
    let channel = |offset: f32| {
        let k = (offset + h * 12.0).rem_euclid(12.0);
        l - a * (-1.0_f32).max((k - 3.0).min(9.0 - k).min(1.0))
    };
    [channel(0.0), channel(8.0), channel(4.0)]
}

/// Converts an sRGB triplet to hue degrees and saturation/lightness percent.
pub fn rgb_to_hsl(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb.map(|value| value.clamp(0.0, 1.0));
    let maximum = r.max(g).max(b);
    let minimum = r.min(g).min(b);
    let delta = maximum - minimum;
    let lightness = (maximum + minimum) * 0.5;
    let saturation = if delta <= f32::EPSILON {
        0.0
    } else {
        delta / (1.0 - (2.0 * lightness - 1.0).abs())
    };
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if maximum == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if maximum == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    [hue, saturation * 100.0, lightness * 100.0]
}

const fn one() -> u64 {
    1
}

/// Stable identifier for a document body.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct BodyId(pub u64);

/// Topological intent of a document body.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum BodyKind {
    /// A closed volumetric body.
    #[default]
    Solid,
    /// An open sheet or shell rendered from both sides.
    Surface,
}

/// Stable identifier for a document construction plane.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct PlaneId(pub u64);

/// Stable identifier for a document construction axis.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct AxisId(pub u64);

/// Stable identifier for a document construction point.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct PointId(pub u64);

/// Stable identifier for an embedded viewport reference image.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ReferenceImageId(pub u64);

/// A portable raster image placed on a document plane.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ReferenceImage {
    /// Stable identifier.
    pub id: ReferenceImageId,
    /// User-facing item name.
    pub name: String,
    /// Original PNG or JPEG file bytes.
    pub bytes: Vec<u8>,
    /// Display width in model millimetres.
    pub width_mm: f64,
    /// Placement plane, initially XY.
    pub plane: SketchPlane,
    /// Plane-local lower-left origin.
    pub origin: DVec2,
    /// Items-panel visibility.
    pub visible: bool,
}

/// A named, bounded datum plane shown in the Items panel and viewport.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConstructionPlane {
    /// Stable identifier.
    pub id: PlaneId,
    /// User-facing item name.
    pub name: String,
    /// Orthonormal world-space plane frame.
    pub plane: SketchPlane,
    /// Whether the datum is drawn and pickable.
    pub visible: bool,
}

/// A named infinite datum axis represented by an origin and unit direction.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConstructionAxis {
    /// Stable identifier.
    pub id: AxisId,
    /// User-facing item name.
    pub name: String,
    /// A point on the axis.
    pub origin: DVec3,
    /// Unit world-space direction.
    pub direction: DVec3,
    /// Whether the datum is drawn and pickable.
    pub visible: bool,
}

/// A named world-space datum point.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ConstructionPoint {
    /// Stable identifier.
    pub id: PointId,
    /// User-facing item name.
    pub name: String,
    /// World-space position.
    pub position: DVec3,
    /// Whether the datum is drawn and pickable.
    pub visible: bool,
}

/// One ordered named value available to later variables and sketch dimensions.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Variable {
    /// ASCII identifier used in expressions.
    pub name: String,
    /// User-authored arithmetic expression.
    pub expr: String,
    /// Last successfully evaluated finite value.
    pub value: f64,
    /// Current evaluation error, if any.
    #[serde(default)]
    pub error: Option<String>,
}

/// Multi-body boolean operation applied left-to-right from a target body.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "t")]
pub enum BooleanOp {
    /// Fuse every tool into the target.
    Union,
    /// Cut every tool from the target.
    Subtract,
    /// Keep the target's common volume with every tool.
    Intersect,
}

/// Parameters for a fillet or chamfer operation.
#[derive(Clone, Debug, PartialEq)]
pub enum DressUp {
    /// Round the selected edges.
    Fillet {
        /// Fillet radius.
        radius: f64,
        /// Optional end radius for a linear variable-radius law.
        end_radius: Option<f64>,
        /// Edge indices in OCCT iteration order.
        edge_indices: Vec<u32>,
    },
    /// Bevel the selected edges.
    Chamfer {
        /// Chamfer distance.
        radius: f64,
        /// Edge indices in OCCT iteration order.
        edge_indices: Vec<u32>,
    },
}

/// Lightweight helical annotation attached to a body without changing its BRep.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct CosmeticThread {
    pub face_index: u32,
    pub external: bool,
    pub origin: DVec3,
    pub axis: DVec3,
    pub radius: f64,
    pub pitch: f64,
    pub depth: f64,
}

/// Whether a thread remains a display annotation or cuts a helical groove.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ThreadMode {
    Cosmetic,
    Modeled,
}

/// One visible or hidden BRep body in the document.
#[derive(Clone)]
pub struct Body {
    /// Stable body identifier.
    pub id: BodyId,
    /// User-facing item name.
    pub name: String,
    /// OpenCASCADE geometry shared between document snapshots.
    pub shape: Arc<Shape>,
    /// Whether the shape represents closed volume or an open sheet.
    pub kind: BodyKind,
    /// Whether this body participates in drawing and picking.
    pub visible: bool,
    /// Persisted viewport surface material.
    pub material: Material,
    /// Cosmetic thread annotations rendered as feature lines.
    pub cosmetic_threads: Vec<CosmeticThread>,
    /// Runtime assembly pose; modeling operations continue to use the unposed BRep.
    pub pose: Mat4,
}

#[cfg(test)]
pub trait IntoTestShape {
    fn into_test_shape(self) -> Shape;
}

#[cfg(test)]
impl IntoTestShape for Shape {
    fn into_test_shape(self) -> Shape {
        self
    }
}

#[cfg(test)]
impl IntoTestShape for Result<Shape, occt::OcctError> {
    fn into_test_shape(self) -> Shape {
        self.expect("test shape construction")
    }
}

/// A rigid transform that can be committed to one or more bodies.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum TransformOp {
    /// Translation in document world units.
    Translate(DVec3),
    /// Rotation about an arbitrary world-space axis line.
    Rotate {
        /// A point on the rotation axis.
        origin: DVec3,
        /// Rotation axis direction.
        axis: DVec3,
        /// Right-handed rotation angle in radians.
        angle_rad: f64,
    },
}

#[derive(Clone)]
struct Snapshot {
    bodies: Vec<Body>,
    joints: Vec<Joint>,
    grounded: HashSet<BodyId>,
    over_constrained: bool,
    sketches: Vec<Sketch>,
    construction_planes: Vec<ConstructionPlane>,
    construction_axes: Vec<ConstructionAxis>,
    construction_points: Vec<ConstructionPoint>,
    reference_images: Vec<ReferenceImage>,
    variables: Vec<Variable>,
    drawing: Drawing,
    active_sketch: Option<SketchId>,
    next_id: u64,
    next_sketch_id: u64,
    next_plane_id: u64,
    next_axis_id: u64,
    next_point_id: u64,
    next_reference_image_id: u64,
    next_joint_id: u64,
    history: Vec<HistoryStep>,
    sketch_dirty: HashSet<SketchId>,
    revision: u64,
}

#[derive(Serialize, Deserialize)]
struct ProjectBody {
    id: BodyId,
    name: String,
    brep: String,
    visible: bool,
    #[serde(default)]
    kind: BodyKind,
    #[serde(default)]
    material: Material,
    #[serde(default)]
    cosmetic_threads: Vec<CosmeticThread>,
}

#[derive(Serialize, Deserialize)]
struct ProjectReferenceImage {
    id: ReferenceImageId,
    name: String,
    data: String,
    width_mm: f64,
    plane: SketchPlane,
    origin: DVec2,
    visible: bool,
}

#[derive(Serialize, Deserialize)]
struct ProjectFile {
    format: String,
    version: u32,
    bodies: Vec<ProjectBody>,
    #[serde(default)]
    joints: Vec<Joint>,
    #[serde(default)]
    grounded: HashSet<BodyId>,
    sketches: Vec<Sketch>,
    construction_planes: Vec<ConstructionPlane>,
    #[serde(default)]
    construction_axes: Vec<ConstructionAxis>,
    #[serde(default)]
    construction_points: Vec<ConstructionPoint>,
    #[serde(default)]
    reference_images: Vec<ProjectReferenceImage>,
    #[serde(default)]
    variables: Vec<Variable>,
    #[serde(default)]
    drawing: Drawing,
    active_sketch: Option<SketchId>,
    next_id: u64,
    next_sketch_id: u64,
    next_plane_id: u64,
    #[serde(default = "one")]
    next_axis_id: u64,
    #[serde(default = "one")]
    next_point_id: u64,
    #[serde(default = "one")]
    next_reference_image_id: u64,
    #[serde(default = "one")]
    next_joint_id: u64,
    history: Vec<HistoryStep>,
    revision: u64,
}

#[derive(Deserialize)]
struct ProjectHeader {
    format: String,
    version: u32,
}

/// A selectable BRep or document-level item.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SelItem {
    /// An entire body.
    Body(BodyId),
    /// A face index in the body's stable OCCT face iteration order.
    Face(BodyId, u32),
    /// An edge index in the body's stable OCCT edge iteration order.
    Edge(BodyId, u32),
    /// A closed profile in a document sketch.
    Profile(SketchId, usize),
    /// A creation-ordered line or circle in a sketch.
    SketchEntity(SketchId, usize),
    /// A document construction plane.
    Plane(PlaneId),
    /// A document construction axis.
    Axis(AxisId),
    /// A document construction point.
    Point(PointId),
}

impl SelItem {
    /// Returns the body owning this selection item.
    pub fn body_id(self) -> Option<BodyId> {
        match self {
            Self::Body(id) | Self::Face(id, _) | Self::Edge(id, _) => Some(id),
            Self::Profile(_, _)
            | Self::SketchEntity(_, _)
            | Self::Plane(_)
            | Self::Axis(_)
            | Self::Point(_) => None,
        }
    }
}

/// Restricts which kind of item viewport clicks can select.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectionFilter {
    /// Prefer nearby edges, otherwise select a face.
    #[default]
    Auto,
    /// Select whole bodies.
    Body,
    /// Select faces only.
    Face,
    /// Select edges only.
    Edge,
}

impl SelectionFilter {
    /// Advances through Auto, Body, Face, and Edge.
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Body,
            Self::Body => Self::Face,
            Self::Face => Self::Edge,
            Self::Edge => Self::Auto,
        }
    }
}

/// Current ordered selection and its active filter.
#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Selected items, without duplicates.
    pub items: Vec<SelItem>,
    /// Active selection kind filter.
    pub filter: SelectionFilter,
}

impl Selection {
    /// Replaces the selection, or toggles `item` when Shift is held.
    pub fn apply(&mut self, item: SelItem, toggle: bool) {
        if toggle {
            if let Some(index) = self.items.iter().position(|current| *current == item) {
                self.items.remove(index);
            } else {
                self.items.push(item);
            }
        } else {
            self.items.clear();
            self.items.push(item);
        }
    }

    /// Clears all selected items without changing the filter.
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// The editable model shared by root chrome and viewport entities.
pub struct Document {
    /// Bodies in Items-panel order.
    pub bodies: Vec<Body>,
    /// Same-document assembly relationships.
    pub joints: Vec<Joint>,
    /// Bodies anchored to their unposed coordinates.
    pub grounded: HashSet<BodyId>,
    /// Solver warning set when a graph edge reaches an already posed body.
    pub over_constrained: bool,
    /// Sketches in Items-panel order.
    pub sketches: Vec<Sketch>,
    /// Construction planes in Items-panel order.
    pub construction_planes: Vec<ConstructionPlane>,
    /// Construction axes in Items-panel order.
    pub construction_axes: Vec<ConstructionAxis>,
    /// Construction points in Items-panel order.
    pub construction_points: Vec<ConstructionPoint>,
    /// Embedded reference images in Items-panel order.
    pub reference_images: Vec<ReferenceImage>,
    /// Ordered named expression values.
    pub variables: Vec<Variable>,
    /// The project's single A4 landscape drawing sheet.
    pub drawing: Drawing,
    /// Sketch currently being edited, if any.
    pub active_sketch: Option<SketchId>,
    next_id: u64,
    next_sketch_id: u64,
    pub(crate) next_plane_id: u64,
    pub(crate) next_axis_id: u64,
    pub(crate) next_point_id: u64,
    pub(crate) next_reference_image_id: u64,
    pub(crate) next_joint_id: u64,
    /// Linear parametric timeline used to rebuild this document.
    pub history: Vec<HistoryStep>,
    sketch_dirty: HashSet<SketchId>,
    recording: bool,
    /// Current selection (not part of undo snapshots).
    pub selection: Selection,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Monotonic signal for geometry/visibility changes requiring GPU upload.
    pub scene_epoch: u64,
    /// Identifier of the current committed document state.
    pub revision: u64,
    revision_clock: u64,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// Creates an empty document.
    pub fn new() -> Self {
        Self {
            bodies: Vec::new(),
            joints: Vec::new(),
            grounded: HashSet::new(),
            over_constrained: false,
            sketches: Vec::new(),
            construction_planes: Vec::new(),
            construction_axes: Vec::new(),
            construction_points: Vec::new(),
            reference_images: Vec::new(),
            variables: Vec::new(),
            drawing: Drawing::default(),
            active_sketch: None,
            next_id: 1,
            next_sketch_id: 1,
            next_plane_id: 1,
            next_axis_id: 1,
            next_point_id: 1,
            next_reference_image_id: 1,
            next_joint_id: 1,
            history: Vec::new(),
            sketch_dirty: HashSet::new(),
            recording: true,
            selection: Selection::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            scene_epoch: 0,
            revision: 0,
            revision_clock: 0,
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            bodies: self.bodies.clone(),
            joints: self.joints.clone(),
            grounded: self.grounded.clone(),
            over_constrained: self.over_constrained,
            sketches: self.sketches.clone(),
            construction_planes: self.construction_planes.clone(),
            construction_axes: self.construction_axes.clone(),
            construction_points: self.construction_points.clone(),
            reference_images: self.reference_images.clone(),
            variables: self.variables.clone(),
            drawing: self.drawing.clone(),
            active_sketch: self.active_sketch,
            next_id: self.next_id,
            next_sketch_id: self.next_sketch_id,
            next_plane_id: self.next_plane_id,
            next_axis_id: self.next_axis_id,
            next_point_id: self.next_point_id,
            next_reference_image_id: self.next_reference_image_id,
            next_joint_id: self.next_joint_id,
            history: self.history.clone(),
            sketch_dirty: self.sketch_dirty.clone(),
            revision: self.revision,
        }
    }

    fn push_undo(&mut self) {
        if self.undo.len() == MAX_SNAPSHOTS {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.redo.clear();
        self.revision_clock = self.revision_clock.wrapping_add(1);
        self.revision = self.revision_clock;
    }

    fn restore(&mut self, snapshot: Snapshot) {
        self.bodies = snapshot.bodies;
        self.joints = snapshot.joints;
        self.grounded = snapshot.grounded;
        self.over_constrained = snapshot.over_constrained;
        self.sketches = snapshot.sketches;
        self.construction_planes = snapshot.construction_planes;
        self.construction_axes = snapshot.construction_axes;
        self.construction_points = snapshot.construction_points;
        self.reference_images = snapshot.reference_images;
        self.variables = snapshot.variables;
        self.drawing = snapshot.drawing;
        self.active_sketch = snapshot.active_sketch;
        self.next_id = snapshot.next_id;
        self.next_sketch_id = snapshot.next_sketch_id;
        self.next_plane_id = snapshot.next_plane_id;
        self.next_axis_id = snapshot.next_axis_id;
        self.next_point_id = snapshot.next_point_id;
        self.next_reference_image_id = snapshot.next_reference_image_id;
        self.next_joint_id = snapshot.next_joint_id;
        self.history = snapshot.history;
        self.sketch_dirty = snapshot.sketch_dirty;
        self.revision = snapshot.revision;
        self.resolve_sketches();
        self.reevaluate_variables();
        self.solve_assembly();
        self.sanitize_selection();
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
    }

    /// Creates an empty document whose mutations do not append history steps.
    pub(crate) fn new_for_replay() -> Self {
        let mut document = Self::new();
        document.recording = false;
        document
    }

    /// Installs the authoritative timeline after a successful replay.
    pub(crate) fn finish_replay(&mut self, history: Vec<HistoryStep>) {
        self.history = history;
        self.sketch_dirty.clear();
        self.recording = true;
        self.undo.clear();
        self.redo.clear();
        self.active_sketch = None;
        self.selection.clear();
    }

    fn record(&mut self, op: HistoryOp) {
        if self.recording {
            self.history.push(HistoryStep::new(op));
        }
    }

    /// Retains an identifier-bearing expression on a numeric parameter of the latest step.
    pub(crate) fn set_last_history_num_expression(&mut self, slot: usize, expression: String) {
        if !expr::contains_identifier(&expression) {
            return;
        }
        let Some(step) = self.history.last_mut() else {
            return;
        };
        let mut current = 0;
        step.op.for_each_num_mut(|parameter| {
            if current == slot {
                parameter.expr = Some(expression.clone());
            }
            current += 1;
        });
    }

    fn flush_sketch_history(&mut self) {
        if !self.recording || self.sketch_dirty.is_empty() {
            return;
        }
        let dirty = std::mem::take(&mut self.sketch_dirty);
        for sketch in self
            .sketches
            .iter()
            .filter(|sketch| dirty.contains(&sketch.id))
        {
            self.history.push(HistoryStep::new(HistoryOp::SketchState {
                sketch: sketch.id,
                entities: sketch.entities.clone(),
                constraints: sketch.constraints.clone(),
                pinned: sketch.pinned.clone(),
            }));
        }
    }

    fn prepare_non_sketch_op(&mut self) {
        self.flush_sketch_history();
    }

    /// Flushes the active sketch's coarse state when sketch mode exits.
    pub fn finish_sketch_mode(&mut self) {
        self.flush_sketch_history();
        self.active_sketch = None;
        self.selection.clear();
    }

    /// Returns a replay-ready copy of the timeline, flushing dirty sketches first.
    pub fn replayable_history(&mut self) -> Vec<HistoryStep> {
        self.flush_sketch_history();
        self.history.clone()
    }

    /// Marks a direct drawing edit dirty without adding it to modeling undo history.
    pub fn drawing_changed(&mut self) {
        self.revision_clock = self.revision_clock.wrapping_add(1);
        self.revision = self.revision_clock;
    }

    /// Saves the complete native project as a versioned `.f3d` JSON file.
    pub fn save_to(&mut self, path: &Path) -> Result<(), String> {
        self.flush_sketch_history();
        let bodies = self
            .bodies
            .iter()
            .map(|body| {
                body.shape
                    .to_brep_data()
                    .map(|bytes| ProjectBody {
                        id: body.id,
                        name: body.name.clone(),
                        brep: BASE64.encode(bytes),
                        visible: body.visible,
                        kind: body.kind,
                        material: body.material,
                        cosmetic_threads: body.cosmetic_threads.clone(),
                    })
                    .map_err(|error| {
                        crate::i18n::tr2(
                            "Could not serialize body {}: {}",
                            &body.name,
                            &error.to_string(),
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let project = ProjectFile {
            format: "free3d".to_owned(),
            version: 1,
            bodies,
            joints: self.joints.clone(),
            grounded: self.grounded.clone(),
            sketches: self.sketches.clone(),
            construction_planes: self.construction_planes.clone(),
            construction_axes: self.construction_axes.clone(),
            construction_points: self.construction_points.clone(),
            reference_images: self
                .reference_images
                .iter()
                .map(|image| ProjectReferenceImage {
                    id: image.id,
                    name: image.name.clone(),
                    data: BASE64.encode(&image.bytes),
                    width_mm: image.width_mm,
                    plane: image.plane,
                    origin: image.origin,
                    visible: image.visible,
                })
                .collect(),
            variables: self.variables.clone(),
            drawing: self.drawing.clone(),
            active_sketch: self.active_sketch,
            next_id: self.next_id,
            next_sketch_id: self.next_sketch_id,
            next_plane_id: self.next_plane_id,
            next_axis_id: self.next_axis_id,
            next_point_id: self.next_point_id,
            next_reference_image_id: self.next_reference_image_id,
            next_joint_id: self.next_joint_id,
            history: self.history.clone(),
            revision: self.revision,
        };
        let json = serde_json::to_vec(&project).map_err(|error| {
            crate::i18n::tr1("Could not encode project file: {}", &error.to_string())
        })?;
        std::fs::write(path, json).map_err(|error| {
            crate::i18n::tr2(
                "Could not write {}: {}",
                &path.display().to_string(),
                &error.to_string(),
            )
        })
    }

    /// Loads a native project and rebuilds all runtime-only document state.
    pub fn load_from(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| {
            crate::i18n::tr2(
                "Could not read {}: {}",
                &path.display().to_string(),
                &error.to_string(),
            )
        })?;
        let header: ProjectHeader = serde_json::from_slice(&bytes).map_err(|error| {
            crate::i18n::tr1("Project file JSON is corrupt: {}", &error.to_string())
        })?;
        if header.format != "free3d" {
            return Err(crate::i18n::tr1(
                "Unsupported file format: {}",
                &header.format.to_string(),
            ));
        }
        if header.version != 1 {
            return Err(crate::i18n::tr1(
                "Unsupported file version: {}",
                &header.version.to_string(),
            ));
        }
        let project: ProjectFile = serde_json::from_slice(&bytes).map_err(|error| {
            crate::i18n::tr1("Project file JSON is corrupt: {}", &error.to_string())
        })?;
        let bodies = project
            .bodies
            .into_iter()
            .map(|body| {
                let bytes = BASE64.decode(&body.brep).map_err(|error| {
                    crate::i18n::tr2(
                        "Body {} has invalid BREP data: {}",
                        &body.name,
                        &error.to_string(),
                    )
                })?;
                let shape = Shape::from_brep_data(&bytes).map_err(|error| {
                    crate::i18n::tr2(
                        "Could not restore body {}: {}",
                        &body.name,
                        &error.to_string(),
                    )
                })?;
                Ok(Body {
                    id: body.id,
                    name: body.name,
                    shape: Arc::new(shape),
                    kind: body.kind,
                    visible: body.visible,
                    material: body.material,
                    cosmetic_threads: body.cosmetic_threads,
                    pose: Mat4::IDENTITY,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let reference_images = project
            .reference_images
            .into_iter()
            .map(|image| {
                BASE64
                    .decode(&image.data)
                    .map(|bytes| ReferenceImage {
                        id: image.id,
                        name: image.name,
                        bytes,
                        width_mm: image.width_mm,
                        plane: image.plane,
                        origin: image.origin,
                        visible: image.visible,
                    })
                    .map_err(|error| {
                        crate::i18n::tr1("Reference image data is invalid: {}", &error.to_string())
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut document = Self {
            bodies,
            joints: project.joints,
            grounded: project.grounded,
            over_constrained: false,
            sketches: project.sketches,
            construction_planes: project.construction_planes,
            construction_axes: project.construction_axes,
            construction_points: project.construction_points,
            reference_images,
            variables: project.variables,
            drawing: project.drawing,
            active_sketch: project.active_sketch,
            next_id: project.next_id,
            next_sketch_id: project.next_sketch_id,
            next_plane_id: project.next_plane_id,
            next_axis_id: project.next_axis_id,
            next_point_id: project.next_point_id,
            next_reference_image_id: project.next_reference_image_id,
            next_joint_id: project.next_joint_id,
            history: project.history,
            sketch_dirty: HashSet::new(),
            recording: true,
            selection: Selection::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            scene_epoch: 1,
            revision: project.revision,
            revision_clock: project.revision,
        };
        document.resolve_sketches();
        document.reevaluate_variables();
        document.solve_assembly();
        Ok(document)
    }

    /// Transactionally replaces and replays the timeline.
    pub fn replace_history(&mut self, steps: Vec<HistoryStep>) -> Result<(), ReplayError> {
        self.flush_sketch_history();
        let replayed = crate::history::replay(&steps)?;
        self.push_undo();
        self.bodies = replayed.bodies;
        self.sketches = replayed.sketches;
        self.construction_planes = replayed.construction_planes;
        self.construction_axes = replayed.construction_axes;
        self.construction_points = replayed.construction_points;
        self.reference_images = replayed.reference_images;
        self.active_sketch = None;
        self.next_id = replayed.next_id;
        self.next_sketch_id = replayed.next_sketch_id;
        self.next_plane_id = replayed.next_plane_id;
        self.next_axis_id = replayed.next_axis_id;
        self.next_point_id = replayed.next_point_id;
        self.next_reference_image_id = replayed.next_reference_image_id;
        self.history = steps;
        self.reevaluate_variables();
        self.sketch_dirty.clear();
        self.selection.clear();
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        Ok(())
    }

    /// Adds an empty sketch and makes it active.
    pub fn add_sketch(&mut self, plane: SketchPlane) -> SketchId {
        self.add_sketch_with_support(plane, None)
    }

    /// Adds a visible construction plane and records it in parametric history.
    pub fn add_construction_plane(&mut self, plane: SketchPlane) -> PlaneId {
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = PlaneId(self.next_plane_id);
        self.next_plane_id += 1;
        let name = format!("{} {}", crate::i18n::t("Construction Plane"), id.0);
        self.construction_planes.push(ConstructionPlane {
            id,
            name,
            plane,
            visible: true,
        });
        self.selection.items = vec![SelItem::Plane(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::AddConstructionPlane { plane });
        id
    }

    /// Creates a construction plane parallel to `base`, offset along its normal.
    pub fn add_offset_construction_plane(&mut self, base: SketchPlane, distance: f64) -> PlaneId {
        let mut plane = base;
        plane.origin += plane.normal() * distance;
        self.add_construction_plane(plane)
    }

    /// Adds a visible construction axis and records it in parametric history.
    pub fn add_construction_axis(&mut self, origin: DVec3, direction: DVec3) -> Option<AxisId> {
        if !origin.is_finite() || !direction.is_finite() || direction.length_squared() < 1.0e-12 {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = AxisId(self.next_axis_id);
        self.next_axis_id += 1;
        let direction = direction.normalize();
        self.construction_axes.push(ConstructionAxis {
            id,
            name: format!("{} {}", crate::i18n::t("Construction Axis"), id.0),
            origin,
            direction,
            visible: true,
        });
        self.selection.items = vec![SelItem::Axis(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::AddConstructionAxis { origin, direction });
        Some(id)
    }

    /// Adds a visible construction point and records it in parametric history.
    pub fn add_construction_point(&mut self, position: DVec3) -> Option<PointId> {
        if !position.is_finite() {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = PointId(self.next_point_id);
        self.next_point_id += 1;
        self.construction_points.push(ConstructionPoint {
            id,
            name: format!("{} {}", crate::i18n::t("Construction Point"), id.0),
            position,
            visible: true,
        });
        self.selection.items = vec![SelItem::Point(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::AddConstructionPoint { position });
        Some(id)
    }

    /// Changes construction-axis visibility.
    pub fn set_construction_axis_visible(&mut self, id: AxisId, visible: bool) {
        if let Some(index) = self
            .construction_axes
            .iter()
            .position(|axis| axis.id == id && axis.visible != visible)
        {
            self.push_undo();
            self.construction_axes[index].visible = visible;
            self.scene_epoch = self.scene_epoch.wrapping_add(1);
        }
    }

    /// Changes construction-point visibility.
    pub fn set_construction_point_visible(&mut self, id: PointId, visible: bool) {
        if let Some(index) = self
            .construction_points
            .iter()
            .position(|point| point.id == id && point.visible != visible)
        {
            self.push_undo();
            self.construction_points[index].visible = visible;
            self.scene_epoch = self.scene_epoch.wrapping_add(1);
        }
    }

    /// Adds an embedded reference image on the XY plane.
    pub fn add_reference_image(
        &mut self,
        name: impl Into<String>,
        bytes: Vec<u8>,
        width_mm: f64,
    ) -> ReferenceImageId {
        let name = name.into();
        let plane = SketchPlane::xy();
        let origin = DVec2::ZERO;
        self.push_undo();
        let id = ReferenceImageId(self.next_reference_image_id);
        self.next_reference_image_id += 1;
        self.reference_images.push(ReferenceImage {
            id,
            name: name.clone(),
            bytes: bytes.clone(),
            width_mm: width_mm.max(0.001),
            plane,
            origin,
            visible: true,
        });
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::AddReferenceImage {
            name,
            data: BASE64.encode(bytes),
            width_mm,
            plane,
            origin,
        });
        id
    }

    pub(crate) fn add_reference_image_replay(
        &mut self,
        name: String,
        data: &str,
        width_mm: f64,
        plane: SketchPlane,
        origin: DVec2,
    ) -> bool {
        let Ok(bytes) = BASE64.decode(data) else {
            return false;
        };
        let id = ReferenceImageId(self.next_reference_image_id);
        self.next_reference_image_id += 1;
        self.reference_images.push(ReferenceImage {
            id,
            name,
            bytes,
            width_mm,
            plane,
            origin,
            visible: true,
        });
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Changes a reference image's visibility.
    pub fn set_reference_image_visible(&mut self, id: ReferenceImageId, visible: bool) {
        let Some(index) = self
            .reference_images
            .iter()
            .position(|image| image.id == id)
        else {
            return;
        };
        if self.reference_images[index].visible == visible {
            return;
        }
        self.push_undo();
        self.reference_images[index].visible = visible;
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
    }

    /// Renames a reference image.
    pub fn rename_reference_image(
        &mut self,
        id: ReferenceImageId,
        name: impl Into<String>,
    ) -> bool {
        let name = name.into();
        if name.trim().is_empty() || !self.reference_images.iter().any(|image| image.id == id) {
            return false;
        }
        self.push_undo();
        self.reference_images
            .iter_mut()
            .find(|image| image.id == id)
            .unwrap()
            .name = name;
        true
    }

    /// Deletes a reference image.
    pub fn remove_reference_image(&mut self, id: ReferenceImageId) -> bool {
        let Some(index) = self
            .reference_images
            .iter()
            .position(|image| image.id == id)
        else {
            return false;
        };
        self.push_undo();
        self.reference_images.remove(index);
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Deletes one construction axis.
    pub fn remove_construction_axis(&mut self, id: AxisId) -> bool {
        let Some(index) = self.construction_axes.iter().position(|axis| axis.id == id) else {
            return false;
        };
        self.push_undo();
        self.construction_axes.remove(index);
        self.selection
            .items
            .retain(|item| *item != SelItem::Axis(id));
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Deletes one construction point.
    pub fn remove_construction_point(&mut self, id: PointId) -> bool {
        let Some(index) = self
            .construction_points
            .iter()
            .position(|point| point.id == id)
        else {
            return false;
        };
        self.push_undo();
        self.construction_points.remove(index);
        self.selection
            .items
            .retain(|item| *item != SelItem::Point(id));
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Changes construction-plane visibility.
    pub fn set_construction_plane_visible(&mut self, id: PlaneId, visible: bool) {
        let Some(index) = self
            .construction_planes
            .iter()
            .position(|plane| plane.id == id && plane.visible != visible)
        else {
            return;
        };
        self.push_undo();
        self.construction_planes[index].visible = visible;
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
    }

    /// Renames one construction plane.
    pub fn rename_construction_plane(&mut self, id: PlaneId, name: impl Into<String>) -> bool {
        let name = name.into();
        let Some(index) = self
            .construction_planes
            .iter()
            .position(|plane| plane.id == id)
        else {
            return false;
        };
        self.push_undo();
        self.construction_planes[index].name = name;
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Deletes one construction plane without deleting sketches based on it.
    pub fn remove_construction_plane(&mut self, id: PlaneId) -> bool {
        let Some(index) = self
            .construction_planes
            .iter()
            .position(|plane| plane.id == id)
        else {
            return false;
        };
        self.push_undo();
        self.construction_planes.remove(index);
        self.selection
            .items
            .retain(|item| *item != SelItem::Plane(id));
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Adds an empty sketch associated with an optional supporting body.
    pub fn add_sketch_with_support(
        &mut self,
        plane: SketchPlane,
        support_body: Option<BodyId>,
    ) -> SketchId {
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = SketchId(self.next_sketch_id);
        self.next_sketch_id += 1;
        let mut sketch = Sketch::new(id, plane);
        sketch.support_body = support_body;
        self.sketches.push(sketch);
        self.active_sketch = Some(id);
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::AddSketch {
            plane,
            support_body,
        });
        id
    }

    /// Adds one or more entities to a sketch as one undoable edit.
    pub fn add_sketch_entities(
        &mut self,
        id: SketchId,
        entities: impl IntoIterator<Item = SketchEntity>,
    ) -> bool {
        let entities: Vec<_> = entities.into_iter().collect();
        if entities.is_empty() || !self.sketches.iter().any(|sketch| sketch.id == id) {
            return false;
        }
        self.add_sketch_entities_with_constraints(id, entities, [])
    }

    /// Projects one body edge, or every boundary edge of a body face, into a sketch.
    ///
    /// Straight samples become lines; other curves become interpolating splines.
    /// Every projected entity is ordinary sketch state flagged as construction.
    pub fn project_to_sketch(&mut self, sketch_id: SketchId, source: SelItem) -> bool {
        let Some(plane) = self
            .sketches
            .iter()
            .find(|sketch| sketch.id == sketch_id)
            .map(|sketch| sketch.plane)
        else {
            return false;
        };
        let (body_id, edge_indices) = match source {
            SelItem::Edge(body, edge) => (body, vec![edge]),
            SelItem::Face(body, face) => {
                let Some(shape) = self.bodies.iter().find(|item| item.id == body) else {
                    return false;
                };
                let Ok(count) = shape.shape.edge_count() else {
                    return false;
                };
                let edges = (0..count)
                    .filter(|edge| {
                        shape
                            .shape
                            .face_contains_edge(face as usize, *edge)
                            .unwrap_or(false)
                    })
                    .map(|edge| edge as u32)
                    .collect();
                (body, edges)
            }
            _ => return false,
        };
        let Some(body) = self.bodies.iter().find(|item| item.id == body_id) else {
            return false;
        };
        let entities: Vec<_> = edge_indices
            .into_iter()
            .filter_map(|edge| {
                let mut points: Vec<_> = body
                    .shape
                    .edge_polyline(edge as usize, 0.1)
                    .ok()?
                    .into_iter()
                    .map(|point| plane.to_local(point))
                    .collect();
                points.dedup_by(|a, b| a.distance_squared(*b) <= 1.0e-20);
                if points.len() < 2 {
                    return None;
                }
                let a = points[0];
                let b = *points.last()?;
                let direction = b - a;
                let collinear = direction.length_squared() > 1.0e-20
                    && points.iter().all(|point| {
                        ((point.x - a.x) * direction.y - (point.y - a.y) * direction.x).abs()
                            / direction.length()
                            <= 1.0e-6
                    });
                Some(SketchItem::construction(if collinear {
                    SketchEntity::Line { a, b }
                } else {
                    SketchEntity::Spline { points }
                }))
            })
            .collect();
        self.add_sketch_items_with_constraints(sketch_id, entities, [])
    }

    /// Adds entities and their already-indexed constraints as one undoable edit.
    pub fn add_sketch_entities_with_constraints(
        &mut self,
        id: SketchId,
        entities: impl IntoIterator<Item = SketchEntity>,
        constraints: impl IntoIterator<Item = Constraint>,
    ) -> bool {
        let entities: Vec<_> = entities.into_iter().map(SketchItem::regular).collect();
        self.add_sketch_items_with_constraints(id, entities, constraints)
    }

    /// Adds already-wrapped sketch items, preserving construction flags.
    pub fn add_sketch_items_with_constraints(
        &mut self,
        id: SketchId,
        entities: impl IntoIterator<Item = SketchItem>,
        constraints: impl IntoIterator<Item = Constraint>,
    ) -> bool {
        let entities: Vec<_> = entities.into_iter().collect();
        if entities.is_empty() || !self.sketches.iter().any(|sketch| sketch.id == id) {
            return false;
        }
        let index = self
            .sketches
            .iter()
            .position(|sketch| sketch.id == id)
            .expect("sketch checked above");
        let mut candidate = self.sketches[index].clone();
        candidate.entities.extend(entities);
        candidate.constraints.extend(constraints);
        let pinned = candidate.pinned.clone();
        let Some(candidate) = Self::solved_candidate(candidate, &pinned) else {
            return false;
        };
        self.push_undo();
        self.sketches[index] = candidate;
        self.sketch_dirty.insert(id);
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Adds generated primitives whose constraint indices are local to the new batch.
    pub fn add_sketch_primitives(
        &mut self,
        id: SketchId,
        entities: Vec<SketchEntity>,
        mut constraints: Vec<Constraint>,
    ) -> bool {
        let Some(base) = self
            .sketches
            .iter()
            .find(|sketch| sketch.id == id)
            .map(|sketch| sketch.entities.len())
        else {
            return false;
        };
        let map: Vec<_> = (0..entities.len())
            .map(|index| Some(base + index))
            .collect();
        if !constraints
            .iter_mut()
            .all(|constraint| constraint.remap_entities(&map))
        {
            return false;
        }
        self.add_sketch_entities_with_constraints(id, entities, constraints)
    }

    /// Toggles construction state for selected entity indices as one sketch edit.
    pub fn toggle_sketch_construction(&mut self, id: SketchId, entities: &[usize]) -> bool {
        if entities.is_empty() {
            return false;
        }
        self.edit_sketch(id, |sketch| {
            let target = !entities
                .iter()
                .filter_map(|&index| sketch.entities.get(index))
                .all(|item| item.construction);
            let mut changed = false;
            for &index in entities {
                if let Some(item) = sketch.entities.get_mut(index) {
                    changed |= item.construction != target;
                    item.construction = target;
                }
            }
            changed
        })
    }

    /// Applies a line-line fillet as one undoable sketch edit.
    pub fn fillet_sketch_lines(
        &mut self,
        id: SketchId,
        first: usize,
        second: usize,
        radius: f64,
    ) -> bool {
        self.edit_sketch(id, |sketch| sketch.fillet_lines(first, second, radius))
    }

    /// Trims the picked sub-segment as one undoable sketch edit.
    pub fn trim_sketch_entity(&mut self, id: SketchId, entity: usize, pick: DVec2) -> bool {
        self.edit_sketch(id, |sketch| sketch.trim(entity, pick))
    }

    /// Extends a line or arc endpoint to the nearest intersection.
    pub fn extend_sketch_entity(&mut self, id: SketchId, entity: usize, pick: DVec2) -> bool {
        self.edit_sketch(id, |sketch| sketch.extend(entity, pick))
    }

    /// Splits a curve at the picked parameter without deleting geometry.
    pub fn break_sketch_entity(&mut self, id: SketchId, entity: usize, pick: DVec2) -> bool {
        self.edit_sketch(id, |sketch| sketch.break_at(entity, pick))
    }

    /// Appends an offset profile copy as one undoable sketch edit.
    pub fn offset_sketch_profile(&mut self, id: SketchId, profile: usize, distance: f64) -> bool {
        self.edit_sketch(id, |sketch| sketch.offset_profile(profile, distance))
    }

    fn edit_sketch(&mut self, id: SketchId, edit: impl FnOnce(&mut Sketch) -> bool) -> bool {
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        let mut candidate = self.sketches[index].clone();
        if !edit(&mut candidate) {
            return false;
        }
        let pinned = candidate.pinned.clone();
        let Some(candidate) = Self::solved_candidate(candidate, &pinned) else {
            return false;
        };
        self.push_undo();
        self.sketches[index] = candidate;
        self.sketch_dirty.insert(id);
        self.selection.clear();
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    fn resolve_sketch(sketch: &mut Sketch) {
        let pinned = sketch.pinned.clone();
        if let Some(candidate) = Self::solved_candidate(sketch.clone(), &pinned) {
            *sketch = candidate;
        } else {
            sketch.defined = vec![false; sketch.entities.len()];
        }
    }

    fn valid_sketch_geometry(sketch: &Sketch) -> bool {
        sketch.entities.iter().all(|entity| match &entity.geo {
            SketchEntity::Line { a, b } => a.distance(*b) > 1.0e-7,
            SketchEntity::Circle { radius, .. } => *radius > 1.0e-7,
            SketchEntity::Ellipse {
                major, minor_ratio, ..
            } => major.length() > 1.0e-7 && *minor_ratio > 1.0e-7,
            SketchEntity::Arc { start, end, mid } => {
                start.distance(*end) > 1.0e-7
                    && ((*mid - *start).perp_dot(*end - *start)).abs() > 1.0e-9
            }
            SketchEntity::Spline { points } => {
                points.len() >= 2
                    && points
                        .windows(2)
                        .all(|pair| pair[0].distance(pair[1]) > 1.0e-7)
            }
            SketchEntity::CvSpline { control, degree } => {
                *degree == 3
                    && control.len() > *degree as usize
                    && control
                        .windows(2)
                        .all(|pair| pair[0].distance(pair[1]) > 1.0e-7)
            }
            SketchEntity::EllipseArc {
                major,
                minor_ratio,
                start_angle,
                end_angle,
                ..
            } => {
                major.length() > 1.0e-7
                    && *minor_ratio > 1.0e-7
                    && (*end_angle - *start_angle).abs() > 1.0e-9
            }
            SketchEntity::Point { at } => at.is_finite(),
        })
    }

    fn solved_candidate(mut sketch: Sketch, pinned: &[usize]) -> Option<Sketch> {
        let result = solve_items(&mut sketch.entities, &sketch.constraints, pinned);
        if !result.converged || !Self::valid_sketch_geometry(&sketch) {
            return None;
        }
        let mut offset = 0;
        sketch.defined = sketch
            .entities
            .iter()
            .map(|item| {
                let count = match &item.geo {
                    SketchEntity::Line { .. } => 4,
                    SketchEntity::Circle { .. } => 3,
                    SketchEntity::Ellipse { .. } => 5,
                    SketchEntity::Arc { .. } => 6,
                    SketchEntity::Spline { points } => points.len() * 2,
                    SketchEntity::CvSpline { control, .. } => control.len() * 2,
                    SketchEntity::EllipseArc { .. } => 7,
                    SketchEntity::Point { .. } => 2,
                };
                let defined = result.determined[offset..offset + count]
                    .iter()
                    .all(|determined| *determined);
                offset += count;
                defined
            })
            .collect();
        sketch.refresh_spline_samples();
        Some(sketch)
    }

    /// Adds constraints atomically, committing only when the cloned sketch converges.
    pub fn add_constraints(
        &mut self,
        id: SketchId,
        constraints: impl IntoIterator<Item = Constraint>,
    ) -> bool {
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        let mut candidate = self.sketches[index].clone();
        candidate.constraints.extend(constraints);
        let pinned = candidate.pinned.clone();
        let Some(candidate) = Self::solved_candidate(candidate, &pinned) else {
            return false;
        };
        self.push_undo();
        self.sketches[index] = candidate;
        self.sketch_dirty.insert(id);
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Adds one constraint transactionally.
    pub fn add_constraint(&mut self, id: SketchId, constraint: Constraint) -> bool {
        self.add_constraints(id, [constraint])
    }

    /// Adds or replaces the default dimension for one entity transactionally.
    #[cfg(test)]
    pub fn set_dimension(&mut self, id: SketchId, entity: usize, value: f64) -> bool {
        if !value.is_finite() || value <= 0.0 {
            return false;
        }
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        let Some(geometry) = self.sketches[index].entities.get(entity) else {
            return false;
        };
        let replacement = match &geometry.geo {
            SketchEntity::Line { .. } => Constraint::Length {
                line: crate::constraint::EntityRef(entity),
                value,
                expr: None,
                error: None,
                reference: false,
            },
            SketchEntity::Circle { .. } => Constraint::Diameter {
                circle: crate::constraint::EntityRef(entity),
                value,
                expr: None,
                error: None,
                reference: false,
            },
            SketchEntity::Arc { .. } => return false,
            SketchEntity::Spline { .. } | SketchEntity::CvSpline { .. } => return false,
            SketchEntity::Ellipse { .. } => return false,
            SketchEntity::EllipseArc { .. } | SketchEntity::Point { .. } => return false,
        };
        self.set_dimension_constraint(id, replacement)
    }

    /// Adds or updates a dimensional constraint, preserving all unrelated dimensions.
    pub fn set_dimension_constraint(&mut self, id: SketchId, replacement: Constraint) -> bool {
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        let same_target = |constraint: &Constraint| match (constraint, &replacement) {
            (Constraint::Length { line: a, .. }, Constraint::Length { line: b, .. }) => a == b,
            (
                Constraint::Radius { circle: a, .. } | Constraint::Diameter { circle: a, .. },
                Constraint::Radius { circle: b, .. } | Constraint::Diameter { circle: b, .. },
            ) => a == b,
            (
                Constraint::Distance { a: a0, b: a1, .. },
                Constraint::Distance { a: b0, b: b1, .. },
            )
            | (
                Constraint::HDistance { a: a0, b: a1, .. },
                Constraint::HDistance { a: b0, b: b1, .. },
            )
            | (
                Constraint::VDistance { a: a0, b: a1, .. },
                Constraint::VDistance { a: b0, b: b1, .. },
            ) => a0 == b0 && a1 == b1,
            (Constraint::Angle { a: a0, b: a1, .. }, Constraint::Angle { a: b0, b: b1, .. }) => {
                a0 == b0 && a1 == b1
            }
            _ => false,
        };
        let valid_value = match &replacement {
            Constraint::Length { value, .. }
            | Constraint::Radius { value, .. }
            | Constraint::Diameter { value, .. }
            | Constraint::Distance { value, .. } => value.is_finite() && *value > 0.0,
            Constraint::HDistance { value, .. } | Constraint::VDistance { value, .. } => {
                value.is_finite()
            }
            Constraint::Angle { degrees, .. } => degrees.is_finite(),
            _ => false,
        };
        if !valid_value {
            return false;
        }
        let mut candidate = self.sketches[index].clone();
        candidate
            .constraints
            .retain(|constraint| !same_target(constraint));
        candidate.constraints.push(replacement);
        let pinned = candidate.pinned.clone();
        let Some(candidate) = Self::solved_candidate(candidate, &pinned) else {
            return false;
        };
        self.push_undo();
        self.sketches[index] = candidate;
        self.sketch_dirty.insert(id);
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Mirrors selected sketch entities across a line as one undoable edit.
    pub fn mirror_sketch_entities(
        &mut self,
        id: SketchId,
        entities: &[usize],
        axis: usize,
    ) -> bool {
        self.edit_sketch(id, |sketch| sketch.mirror_entities(entities, axis))
    }

    /// Creates an in-plane linear sketch pattern as one undoable edit.
    pub fn pattern_sketch_linear(
        &mut self,
        id: SketchId,
        entities: &[usize],
        direction: DVec2,
        count: usize,
        spacing: f64,
    ) -> bool {
        self.edit_sketch(id, |sketch| {
            sketch.linear_pattern(entities, direction, count, spacing)
        })
    }

    /// Creates an in-plane circular sketch pattern as one undoable edit.
    pub fn pattern_sketch_circular(
        &mut self,
        id: SketchId,
        entities: &[usize],
        center: DVec2,
        count: usize,
    ) -> bool {
        self.edit_sketch(id, |sketch| {
            sketch.circular_pattern(entities, center, count)
        })
    }

    fn entity_parameter_start(entities: &[SketchItem], entity: usize) -> Option<usize> {
        (entity < entities.len()).then(|| {
            entities[..entity]
                .iter()
                .map(|curve| match &curve.geo {
                    SketchEntity::Line { .. } => 4,
                    SketchEntity::Circle { .. } => 3,
                    SketchEntity::Ellipse { .. } => 5,
                    SketchEntity::Arc { .. } => 6,
                    SketchEntity::Spline { points } => points.len() * 2,
                    SketchEntity::CvSpline { control, .. } => control.len() * 2,
                    SketchEntity::EllipseArc { .. } => 7,
                    SketchEntity::Point { .. } => 2,
                })
                .sum()
        })
    }

    /// Toggles complete solver pinning for selected sketch entities.
    pub fn toggle_sketch_fix(&mut self, id: SketchId, entities: &[usize]) -> bool {
        if entities.is_empty() {
            return false;
        }
        self.edit_sketch(id, |sketch| {
            let total: usize = sketch
                .entities
                .iter()
                .map(|item| match &item.geo {
                    SketchEntity::Line { .. } => 4,
                    SketchEntity::Circle { .. } => 3,
                    SketchEntity::Ellipse { .. } => 5,
                    SketchEntity::Arc { .. } => 6,
                    SketchEntity::Spline { points } => points.len() * 2,
                    SketchEntity::CvSpline { control, .. } => control.len() * 2,
                    SketchEntity::EllipseArc { .. } => 7,
                    SketchEntity::Point { .. } => 2,
                })
                .sum();
            let ranges: Vec<_> = entities
                .iter()
                .filter_map(|&entity| {
                    let start = Self::entity_parameter_start(&sketch.entities, entity)?;
                    let end =
                        Self::entity_parameter_start(&sketch.entities, entity + 1).unwrap_or(total);
                    Some(start..end)
                })
                .collect();
            let all_fixed = ranges
                .iter()
                .cloned()
                .flatten()
                .all(|p| sketch.pinned.contains(&p));
            for parameter in ranges.into_iter().flatten() {
                if all_fixed {
                    sketch.pinned.retain(|p| *p != parameter);
                } else if !sketch.pinned.contains(&parameter) {
                    sketch.pinned.push(parameter);
                }
            }
            sketch.pinned.sort_unstable();
            true
        })
    }

    /// Computes and commits one live drag frame from an immutable drag-start sketch.
    pub fn preview_sketch_drag(
        &mut self,
        id: SketchId,
        entity: usize,
        delta: DVec2,
        start: &Sketch,
    ) -> bool {
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        let Some(parameter) = Self::entity_parameter_start(&start.entities, entity) else {
            return false;
        };
        let mut candidate = start.clone();
        let pinned = match &mut candidate.entities[entity].geo {
            SketchEntity::Line { a, b } => {
                *a += delta;
                *b += delta;
                vec![parameter, parameter + 1, parameter + 2, parameter + 3]
            }
            SketchEntity::Circle { center, .. } => {
                *center += delta;
                vec![parameter, parameter + 1]
            }
            SketchEntity::Ellipse { center, .. } => {
                *center += delta;
                vec![parameter, parameter + 1]
            }
            SketchEntity::Arc { start, end, mid } => {
                *start += delta;
                *end += delta;
                *mid += delta;
                (parameter..parameter + 6).collect()
            }
            SketchEntity::Spline { points } => {
                for point in points.iter_mut() {
                    *point += delta;
                }
                (parameter..parameter + points.len() * 2).collect()
            }
            SketchEntity::CvSpline { control, .. } => {
                for point in control.iter_mut() {
                    *point += delta;
                }
                (parameter..parameter + control.len() * 2).collect()
            }
            SketchEntity::EllipseArc { center, .. } => {
                *center += delta;
                vec![parameter, parameter + 1]
            }
            SketchEntity::Point { at } => {
                *at += delta;
                vec![parameter, parameter + 1]
            }
        };
        let Some(candidate) = Self::solved_candidate(candidate, &pinned) else {
            return false;
        };
        self.sketches[index] = candidate;
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Finishes one drag with an unpinned solve and one undo snapshot, or cancels it.
    pub fn finish_sketch_drag(&mut self, id: SketchId, start: Sketch, commit: bool) -> bool {
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        if !commit {
            self.sketches[index] = start;
            self.scene_epoch = self.scene_epoch.wrapping_add(1);
            return true;
        }
        let current = self.sketches[index].clone();
        let pinned = current.pinned.clone();
        let Some(candidate) = Self::solved_candidate(current, &pinned) else {
            self.sketches[index] = start;
            self.scene_epoch = self.scene_epoch.wrapping_add(1);
            return false;
        };
        let mut before = self.snapshot();
        before.sketches[index] = start;
        if self.undo.len() == MAX_SNAPSHOTS {
            self.undo.remove(0);
        }
        self.undo.push(before);
        self.redo.clear();
        self.revision_clock = self.revision_clock.wrapping_add(1);
        self.revision = self.revision_clock;
        self.sketches[index] = candidate;
        self.sketch_dirty.insert(id);
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Re-solves every sketch and refreshes its defined-state cache.
    pub fn resolve_sketches(&mut self) {
        for sketch in &mut self.sketches {
            Self::resolve_sketch(sketch);
        }
    }

    /// Returns whether `name` is a valid variable identifier.
    pub fn valid_variable_name(name: &str) -> bool {
        let mut bytes = name.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    }

    /// Adds `varN = 0` as one undoable edit and returns its row index.
    pub fn add_variable(&mut self) -> usize {
        let mut suffix = 1;
        let name = loop {
            let candidate = format!("var{suffix}");
            if self
                .variables
                .iter()
                .all(|variable| variable.name != candidate)
            {
                break candidate;
            }
            suffix += 1;
        };
        self.push_undo();
        self.variables.push(Variable {
            name,
            expr: "0".to_owned(),
            value: 0.0,
            error: None,
        });
        let index = self.variables.len() - 1;
        self.reevaluate_variables_for(Some(index));
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        index
    }

    /// Replaces one variable row as one undoable panel edit.
    pub fn update_variable(&mut self, index: usize, name: String, expression: String) -> bool {
        if !Self::valid_variable_name(&name)
            || self
                .variables
                .iter()
                .enumerate()
                .any(|(other, variable)| other != index && variable.name == name)
        {
            return false;
        }
        let Some(variable) = self.variables.get(index) else {
            return false;
        };
        if variable.name == name && variable.expr == expression {
            return false;
        }
        self.push_undo();
        let variable = &mut self.variables[index];
        variable.name = name;
        variable.expr = expression;
        self.reevaluate_variables_for(Some(index));
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Deletes one variable row as an undoable edit.
    pub fn remove_variable(&mut self, index: usize) -> bool {
        if index >= self.variables.len() {
            return false;
        }
        self.push_undo();
        self.variables.remove(index);
        self.reevaluate_variables();
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Re-evaluates ordered variables and every expression-driven sketch dimension.
    ///
    /// Failed variable expressions retain their last value. Sketches are solved
    /// transactionally, so a failed solve retains its previous geometry and expression.
    pub fn reevaluate_variables(&mut self) {
        self.reevaluate_variables_for(None);
    }

    fn reevaluate_variables_for(&mut self, edited_variable: Option<usize>) {
        self.flush_sketch_history();
        let mut values = HashMap::<String, f64>::new();
        let mut last_changed = None;
        for (index, variable) in self.variables.iter_mut().enumerate() {
            let resolver = |name: &str| values.get(name).copied();
            match expr::eval_with(&variable.expr, &resolver) {
                Some(value) => {
                    if variable.value != value {
                        last_changed = Some(index);
                    }
                    variable.value = value;
                    variable.error = None;
                }
                None => {
                    variable.error = Some(
                        expr::first_unknown_identifier(&variable.expr, &resolver)
                            .map(|name| crate::i18n::tr1("Undefined variable {}", &name))
                            .unwrap_or_else(|| crate::i18n::t("Expression is invalid").to_owned()),
                    );
                }
            }
            values.insert(variable.name.clone(), variable.value);
        }

        let resolver = |name: &str| values.get(name).copied();
        let mut steps = self.history.clone();
        let mut operation_changed = false;
        let mut operation_error = None;
        for step in &mut steps {
            step.op.for_each_num_mut(|parameter| {
                let Some(source) = parameter.expr.as_deref() else {
                    return;
                };
                match expr::eval_with(source, &resolver) {
                    Some(value) if parameter.value != value => {
                        parameter.value = value;
                        operation_changed = true;
                    }
                    Some(_) => {}
                    None => {
                        operation_error = Some(
                            expr::first_unknown_identifier(source, &resolver)
                                .map(|name| {
                                    crate::i18n::tr1(
                                        "Feature expression references undefined variable {}",
                                        &name,
                                    )
                                })
                                .unwrap_or_else(|| {
                                    crate::i18n::t("Feature expression is invalid").to_owned()
                                }),
                        );
                    }
                }
            });
        }
        if let Some(error) = operation_error {
            if let Some(variable) = edited_variable
                .or(last_changed)
                .and_then(|index| self.variables.get_mut(index))
            {
                variable.error = Some(error);
            }
            return;
        }
        if operation_changed {
            match crate::history::replay(&steps) {
                Ok(replayed) => {
                    self.bodies = replayed.bodies;
                    self.sketches = replayed.sketches;
                    self.construction_planes = replayed.construction_planes;
                    self.construction_axes = replayed.construction_axes;
                    self.construction_points = replayed.construction_points;
                    self.reference_images = replayed.reference_images;
                    self.next_id = replayed.next_id;
                    self.next_sketch_id = replayed.next_sketch_id;
                    self.next_plane_id = replayed.next_plane_id;
                    self.next_axis_id = replayed.next_axis_id;
                    self.next_point_id = replayed.next_point_id;
                    self.next_reference_image_id = replayed.next_reference_image_id;
                    self.history = steps;
                    self.sketch_dirty.clear();
                    self.selection.clear();
                    self.scene_epoch = self.scene_epoch.wrapping_add(1);
                }
                Err(error) => {
                    if let Some(variable) = edited_variable
                        .or(last_changed)
                        .and_then(|index| self.variables.get_mut(index))
                    {
                        variable.error = Some(crate::i18n::tr2(
                            "History step {} recompute failed: {}",
                            &(error.step_index + 1).to_string(),
                            &error.message,
                        ));
                    }
                    return;
                }
            }
        }

        for index in 0..self.sketches.len() {
            if !self.sketches[index]
                .constraints
                .iter()
                .any(|constraint| constraint.expression().is_some())
            {
                continue;
            }
            let id = self.sketches[index].id;
            let mut candidate = self.sketches[index].clone();
            let mut expression_indices = Vec::new();
            for (constraint_index, constraint) in candidate.constraints.iter_mut().enumerate() {
                let Some(source) = constraint.expression().map(str::to_owned) else {
                    continue;
                };
                expression_indices.push(constraint_index);
                if let Some(value) = expr::eval_with(&source, &resolver) {
                    constraint.set_expression_result(Some(value), None);
                } else {
                    let message = expr::first_unknown_identifier(&source, &resolver)
                        .map(|name| crate::i18n::tr1("Undefined variable {}", &name))
                        .unwrap_or_else(|| crate::i18n::t("Expression is invalid").to_owned());
                    constraint.set_expression_result(None, Some(message));
                }
            }
            let attempted_constraints = candidate.constraints.clone();
            let pins = candidate.pinned.clone();
            if let Some(candidate) = Self::solved_candidate(candidate, &pins) {
                self.sketches[index] = candidate;
            } else {
                self.sketches[index].constraints = attempted_constraints;
                for constraint_index in expression_indices {
                    self.sketches[index].constraints[constraint_index].set_expression_error(Some(
                        crate::i18n::t("Constraint solver did not converge").to_owned(),
                    ));
                }
            }
            self.sketch_dirty.insert(id);
        }
    }

    /// Replaces a sketch's editable state during replay and validates it.
    pub(crate) fn apply_sketch_state(
        &mut self,
        id: SketchId,
        entities: Vec<SketchItem>,
        constraints: Vec<Constraint>,
        pinned: Vec<usize>,
    ) -> bool {
        let Some(index) = self.sketches.iter().position(|sketch| sketch.id == id) else {
            return false;
        };
        let mut candidate = self.sketches[index].clone();
        candidate.entities = entities;
        candidate.constraints = constraints;
        candidate.pinned = pinned;
        let pins = candidate.pinned.clone();
        let Some(candidate) = Self::solved_candidate(candidate, &pins) else {
            return false;
        };
        self.push_undo();
        self.sketches[index] = candidate;
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Changes a sketch's Items-panel visibility.
    pub fn set_sketch_visible(&mut self, id: SketchId, visible: bool) {
        if !self
            .sketches
            .iter()
            .any(|sketch| sketch.id == id && sketch.visible != visible)
        {
            return;
        }
        self.push_undo();
        if let Some(sketch) = self.sketches.iter_mut().find(|sketch| sketch.id == id) {
            sketch.visible = visible;
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
    }

    /// Adds a visible body and returns its stable id.
    #[cfg(test)]
    pub fn add_body(&mut self, name: impl Into<String>, shape: impl IntoTestShape) -> BodyId {
        self.prepare_non_sketch_op();
        self.push_undo();
        self.add_body_raw(name.into(), shape.into_test_shape())
    }

    fn add_body_raw(&mut self, name: String, shape: Shape) -> BodyId {
        let id = BodyId(self.next_id);
        self.next_id += 1;
        self.bodies.push(Body {
            id,
            name,
            shape: Arc::new(shape),
            kind: BodyKind::Solid,
            visible: true,
            material: Material::default(),
            cosmetic_threads: Vec::new(),
            pose: Mat4::IDENTITY,
        });
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        id
    }

    /// Adds and records one replayable built-in primitive.
    ///
    /// The new body becomes the selection (Shapr3D convention: inserted
    /// geometry is immediately manipulable).
    pub fn add_primitive(&mut self, kind: PrimitiveKind) -> BodyId {
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = self.add_body_raw(self.next_name(kind.name()), kind.shape());
        self.record(HistoryOp::AddPrimitive { kind });
        self.selection.items = vec![SelItem::Body(id)];
        id
    }

    /// Reads supported external geometry, splitting STEP/IGES solids into bodies.
    pub fn import_file(&mut self, path: &Path) -> Result<BodyId, String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension == "dxf" {
            let entities = crate::io_formats::read_dxf(path)?;
            self.prepare_non_sketch_op();
            self.push_undo();
            let id = SketchId(self.next_sketch_id);
            self.next_sketch_id += 1;
            let mut sketch = Sketch::new(id, SketchPlane::xy());
            sketch.entities = entities;
            sketch.refresh_spline_samples();
            self.sketches.push(sketch);
            self.active_sketch = Some(id);
            self.scene_epoch = self.scene_epoch.wrapping_add(1);
            self.record(HistoryOp::ImportFile {
                path: path.to_path_buf(),
            });
            return Ok(BodyId(id.0));
        }
        let shape = match extension.as_str() {
            "step" | "stp" => Shape::read_step(path),
            "iges" | "igs" => Shape::read_iges(path),
            "stl" => Shape::read_stl(path),
            _ => {
                return Err(
                    "import path must end in .step, .stp, .stl, .iges, .igs, or .dxf".to_owned(),
                );
            }
        }
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .unwrap_or("Imported")
            .to_owned();
        self.add_imported_step(path.to_path_buf(), stem, shape)
    }

    /// Backward-compatible STEP/STL import entry point.
    pub fn import_step(&mut self, path: &Path) -> Result<BodyId, String> {
        self.import_file(path)
    }

    /// Adds an already-loaded STEP shape while retaining its replay path.
    pub fn add_imported_step(
        &mut self,
        path: PathBuf,
        stem: String,
        shape: Shape,
    ) -> Result<BodyId, String> {
        let is_solid_exchange = matches!(
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref(),
            Some("step" | "stp" | "iges" | "igs")
        );
        let mut shapes = if is_solid_exchange {
            shape.solids().map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        if shapes.is_empty() {
            shapes.push(shape);
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let split = shapes.len() > 1;
        let mut ids = Vec::with_capacity(shapes.len());
        for (index, shape) in shapes.into_iter().enumerate() {
            let requested = if split {
                format!("{stem} {}", index + 1)
            } else {
                stem.clone()
            };
            let name = if self.bodies.iter().all(|body| body.name != requested) {
                requested
            } else {
                self.next_name(&requested)
            };
            ids.push(self.add_body_raw(name, shape));
        }
        let id = ids[0];
        self.selection.items = ids.into_iter().map(SelItem::Body).collect();
        self.record(HistoryOp::ImportFile { path });
        Ok(id)
    }

    /// Writes visible bodies or sketches in the format selected by extension.
    pub fn export(&mut self, path: &Path) -> Result<(), String> {
        self.flush_sketch_history();
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension == "dxf" {
            let sketches: Vec<_> = self
                .active_sketch
                .and_then(|id| self.sketches.iter().find(|sketch| sketch.id == id))
                .map_or_else(|| self.sketches.iter().collect(), |sketch| vec![sketch]);
            if sketches.is_empty() {
                return Err("document has no sketch to export".to_owned());
            }
            return crate::io_formats::write_dxf(path, &sketches)
                .map_err(|error| format!("failed to write {}: {error}", path.display()));
        }
        let visible: Vec<&Shape> = self
            .bodies
            .iter()
            .filter(|body| body.visible)
            .map(|body| body.shape.as_ref())
            .collect();
        if visible.is_empty() {
            return Err("document has no visible bodies to export".to_owned());
        }
        if matches!(extension.as_str(), "obj" | "gltf" | "glb" | "3mf") {
            let meshes = crate::io_formats::meshes(&visible)?;
            let result = match extension.as_str() {
                "obj" => crate::io_formats::write_obj(path, &meshes),
                "gltf" => crate::io_formats::write_gltf(path, &meshes, false),
                "glb" => crate::io_formats::write_gltf(path, &meshes, true),
                "3mf" => crate::io_formats::write_3mf(path, &meshes),
                _ => unreachable!(),
            };
            return result.map_err(|error| format!("failed to write {}: {error}", path.display()));
        }
        match extension.as_str() {
            "step" | "stp" => Shape::write_step_refs(visible.iter().copied(), path),
            "iges" | "igs" => Shape::write_iges_refs(visible.iter().copied(), path),
            "stl" => Shape::compound(
                visible
                    .iter()
                    .map(|shape| shape.try_clone())
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?,
            )
            .and_then(|shape| shape.write_stl(path, 0.1)),
            _ => return Err(
                "unsupported export extension (use step, stl, iges, obj, gltf, glb, 3mf, or dxf)"
                    .to_owned(),
            ),
        }
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
    }

    /// Removes all bodies whose ids occur in `ids`.
    pub fn remove_bodies(&mut self, ids: &[BodyId]) {
        let existing: Vec<_> = self
            .bodies
            .iter()
            .filter(|body| ids.contains(&body.id))
            .map(|body| body.id)
            .collect();
        if existing.is_empty() {
            return;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies.retain(|body| !existing.contains(&body.id));
        self.joints
            .retain(|joint| !existing.contains(&joint.a.0) && !existing.contains(&joint.b.0));
        self.grounded.retain(|id| !existing.contains(id));
        self.solve_assembly();
        self.sanitize_selection();
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::DeleteBodies { ids: existing });
    }

    pub(crate) fn remove_bodies_checked(&mut self, ids: &[BodyId]) -> bool {
        if ids.is_empty()
            || ids
                .iter()
                .any(|id| !self.bodies.iter().any(|body| body.id == *id))
        {
            return false;
        }
        self.remove_bodies(ids);
        true
    }

    /// Changes a body's visibility as an undoable mutation.
    pub fn set_visible(&mut self, id: BodyId, visible: bool) {
        if !self
            .bodies
            .iter()
            .any(|body| body.id == id && body.visible != visible)
        {
            return;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        if let Some(body) = self.bodies.iter_mut().find(|body| body.id == id) {
            body.visible = visible;
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::SetVisible { id, visible });
    }

    pub(crate) fn set_visible_checked(&mut self, id: BodyId, visible: bool) -> bool {
        if !self.bodies.iter().any(|body| body.id == id) {
            return false;
        }
        self.set_visible(id, visible);
        true
    }

    /// Renames a body as an undoable mutation.
    pub fn rename(&mut self, id: BodyId, name: impl Into<String>) {
        let name = name.into();
        if !self
            .bodies
            .iter()
            .any(|body| body.id == id && body.name != name)
        {
            return;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        if let Some(body) = self.bodies.iter_mut().find(|body| body.id == id) {
            body.name = name.clone();
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Rename { id, name });
    }

    pub(crate) fn rename_checked(&mut self, id: BodyId, name: impl Into<String>) -> bool {
        if !self.bodies.iter().any(|body| body.id == id) {
            return false;
        }
        self.rename(id, name);
        true
    }

    /// Changes a body's material as one undoable, replayable mutation.
    pub fn set_material(&mut self, id: BodyId, material: Material) {
        let material = material.clamped();
        if !self
            .bodies
            .iter()
            .any(|body| body.id == id && body.material != material)
        {
            return;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        if let Some(body) = self.bodies.iter_mut().find(|body| body.id == id) {
            body.material = material;
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::SetMaterial { body: id, material });
    }

    pub(crate) fn set_material_checked(&mut self, id: BodyId, material: Material) -> bool {
        if !self.bodies.iter().any(|body| body.id == id) {
            return false;
        }
        self.set_material(id, material);
        true
    }

    /// Adds an assembly joint as one undoable, replayable mutation.
    pub fn add_joint(&mut self, mut joint: Joint) -> Option<JointId> {
        if joint.a.0 == joint.b.0
            || !self.bodies.iter().any(|body| body.id == joint.a.0)
            || !self.bodies.iter().any(|body| body.id == joint.b.0)
        {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        if joint.id.0 == 0 {
            joint.id = JointId(self.next_joint_id);
        }
        self.next_joint_id = self.next_joint_id.max(joint.id.0.saturating_add(1));
        let id = joint.id;
        self.joints.push(joint.clone());
        self.solve_assembly();
        self.record(HistoryOp::AddJoint { joint });
        Some(id)
    }

    /// Deletes one joint.
    pub fn delete_joint(&mut self, id: JointId) -> bool {
        if !self.joints.iter().any(|joint| joint.id == id) {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.joints.retain(|joint| joint.id != id);
        self.solve_assembly();
        self.record(HistoryOp::DeleteJoint { id });
        true
    }

    /// Updates the primary and secondary drive values, clamps limits, then re-solves.
    pub fn set_joint_value(&mut self, id: JointId, value: f64, value2: f64) -> bool {
        let Some(index) = self.joints.iter().position(|joint| joint.id == id) else {
            return false;
        };
        if self.joints[index].value == value && self.joints[index].value2 == value2 {
            return true;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.joints[index].value = value;
        self.joints[index].value2 = value2;
        self.solve_assembly();
        let joint = &self.joints[index];
        self.record(HistoryOp::SetJointValue {
            id,
            value: joint.value,
            value2: joint.value2,
        });
        true
    }

    /// Grounds or ungrounds a body and re-solves the assembly.
    pub fn set_grounded(&mut self, body: BodyId, grounded: bool) -> bool {
        if !self.bodies.iter().any(|item| item.id == body) {
            return false;
        }
        if self.grounded.contains(&body) == grounded {
            return true;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        if grounded {
            self.grounded.insert(body);
        } else {
            self.grounded.remove(&body);
        }
        self.solve_assembly();
        self.record(HistoryOp::SetGrounded { body, grounded });
        true
    }

    /// Convenience toggle used by the adaptive ground action.
    pub fn toggle_grounded(&mut self, body: BodyId) -> bool {
        self.set_grounded(body, !self.grounded.contains(&body))
    }

    /// Applies one rigid transform to all matching bodies as one undo step.
    pub fn apply_transform(&mut self, ids: &[BodyId], op: TransformOp) -> Vec<BodyId> {
        let changed: Vec<_> = self
            .bodies
            .iter()
            .filter(|body| ids.contains(&body.id))
            .map(|body| body.id)
            .collect();
        if changed.is_empty() {
            return changed;
        }
        let transformed = self
            .bodies
            .iter()
            .filter(|body| changed.contains(&body.id))
            .map(|body| {
                let shape = match op {
                    TransformOp::Translate(delta) => body.shape.translated(delta),
                    TransformOp::Rotate {
                        origin,
                        axis,
                        angle_rad,
                    } => body
                        .shape
                        .rotated(origin, axis.normalize_or_zero(), angle_rad),
                };
                Some((body.id, shape.ok()?))
            })
            .collect::<Option<Vec<_>>>();
        let Some(transformed) = transformed else {
            return Vec::new();
        };
        self.prepare_non_sketch_op();
        self.push_undo();
        for (id, shape) in transformed {
            self.bodies
                .iter_mut()
                .find(|body| body.id == id)
                .unwrap()
                .shape = Arc::new(shape);
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Transform {
            ids: changed.clone(),
            op,
        });
        changed
    }

    /// Keeps the source bodies and creates copies at transform multiples `1..count`.
    ///
    /// A count of three therefore selects the original plus copies at one and two
    /// transform steps. The whole pattern is stored as one deterministic history op.
    pub fn apply_multi_transform(
        &mut self,
        ids: &[BodyId],
        op: TransformOp,
        count: usize,
    ) -> Vec<BodyId> {
        if !(2..=5).contains(&count) {
            return Vec::new();
        }
        self.prepare_non_sketch_op();
        let sources = self.unique_bodies(ids);
        let source_ids: Vec<_> = sources.iter().map(|body| body.id).collect();
        if source_ids.is_empty() {
            return Vec::new();
        }
        let copies = (1..count)
            .flat_map(|multiple| {
                sources.iter().filter_map(move |body| {
                    let shape = match op {
                        TransformOp::Translate(delta) => {
                            body.shape.translated(delta * multiple as f64).ok()?
                        }
                        TransformOp::Rotate {
                            origin,
                            axis,
                            angle_rad,
                        } => body
                            .shape
                            .rotated(origin, axis, angle_rad * multiple as f64)
                            .ok()?,
                    };
                    Some((format!("{} {}", body.name, multiple + 1), shape))
                })
            })
            .collect();
        let copies = self.add_transformed_copies(copies, &source_ids);
        if !copies.is_empty() {
            self.record(HistoryOp::MultiTransform {
                ids: source_ids,
                op,
                count: count as u32,
            });
        }
        copies
    }

    /// Uniformly scales all matching bodies about `pivot` as one undoable operation.
    pub fn apply_scale(&mut self, ids: &[BodyId], factor: f64, pivot: DVec3) -> Vec<BodyId> {
        if !factor.is_finite() {
            return Vec::new();
        }
        let factor = factor.clamp(0.01, 100.0);
        let changed: Vec<_> = self
            .bodies
            .iter()
            .filter(|body| ids.contains(&body.id))
            .map(|body| body.id)
            .collect();
        if changed.is_empty() || (factor - 1.0).abs() <= f64::EPSILON {
            return Vec::new();
        }
        let scaled = self
            .bodies
            .iter()
            .filter(|body| changed.contains(&body.id))
            .map(|body| Some((body.id, body.shape.scaled(pivot, factor).ok()?)))
            .collect::<Option<Vec<_>>>();
        let Some(scaled) = scaled else {
            return Vec::new();
        };
        self.prepare_non_sketch_op();
        self.push_undo();
        for (id, shape) in scaled {
            self.bodies
                .iter_mut()
                .find(|body| body.id == id)
                .unwrap()
                .shape = Arc::new(shape);
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Scale {
            ids: changed.clone(),
            factor: factor.into(),
            pivot,
        });
        changed
    }

    /// Translates every body after the first to match its bounding-box center on chosen axes.
    ///
    /// Each translation is intentionally recorded as an ordinary Transform history step.
    pub fn apply_align(&mut self, ids: &[BodyId], axes: [bool; 3]) -> Vec<BodyId> {
        let Some(reference) = ids
            .first()
            .and_then(|id| self.bodies.iter().find(|b| b.id == *id))
        else {
            return Vec::new();
        };
        if ids.len() < 2 || !axes.iter().any(|enabled| *enabled) {
            return Vec::new();
        }
        let Ok((minimum, maximum)) = reference.shape.aabb() else {
            return Vec::new();
        };
        let target = (minimum + maximum) * 0.5;
        let moves: Vec<_> = ids[1..]
            .iter()
            .filter_map(|id| {
                let body = self.bodies.iter().find(|body| body.id == *id)?;
                let (minimum, maximum) = body.shape.aabb().ok()?;
                let center = (minimum + maximum) * 0.5;
                let raw = target - center;
                Some((
                    *id,
                    DVec3::new(
                        if axes[0] { raw.x } else { 0.0 },
                        if axes[1] { raw.y } else { 0.0 },
                        if axes[2] { raw.z } else { 0.0 },
                    ),
                ))
            })
            .collect();
        let mut changed = Vec::new();
        for (id, delta) in moves {
            if delta.length_squared() > 1.0e-20 {
                changed.extend(self.apply_transform(&[id], TransformOp::Translate(delta)));
            }
        }
        changed
    }

    /// Splits one body with a world-Y plane, replacing it with named A/B halves.
    pub fn apply_split(&mut self, id: BodyId, y: f64) -> Vec<BodyId> {
        let Some((index, body)) = self
            .bodies
            .iter()
            .enumerate()
            .find(|(_, body)| body.id == id)
        else {
            return Vec::new();
        };
        let Ok((minimum, maximum)) = body.shape.aabb() else {
            return Vec::new();
        };
        if !y.is_finite() || y <= minimum.y + 1.0e-8 || y >= maximum.y - 1.0e-8 {
            return Vec::new();
        }
        let span = (maximum - minimum).max(DVec3::ONE);
        let margin = span.max_element() * 10.0;
        let cover_min = minimum - DVec3::splat(margin);
        let cover_max = maximum + DVec3::splat(margin);
        let Ok(right_box) =
            Shape::box_from_corners(DVec3::new(cover_min.x, y, cover_min.z), cover_max)
        else {
            return Vec::new();
        };
        let Ok(left_box) =
            Shape::box_from_corners(cover_min, DVec3::new(cover_max.x, y, cover_max.z))
        else {
            return Vec::new();
        };
        let Ok(left) = body.shape.cut(&right_box) else {
            return Vec::new();
        };
        let Ok(right) = body.shape.cut(&left_box) else {
            return Vec::new();
        };
        if !Self::renderable(&left) || !Self::renderable(&right) {
            return Vec::new();
        }
        let name = body.name.clone();
        let visible = body.visible;
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies.remove(index);
        let a = self.add_body_raw(format!("{name} A"), left);
        let b = self.add_body_raw(format!("{name} B"), right);
        if !visible {
            for body in self
                .bodies
                .iter_mut()
                .filter(|body| body.id == a || body.id == b)
            {
                body.visible = false;
            }
        }
        self.selection.items = vec![SelItem::Body(a), SelItem::Body(b)];
        self.record(HistoryOp::Split { body: id, y });
        vec![a, b]
    }

    /// Revolves a closed sketch profile around an explicit world-space axis.
    pub fn apply_revolve(
        &mut self,
        source: SelItem,
        axis_origin: DVec3,
        axis_direction: DVec3,
        angle_degrees: f64,
        mode: ExtrudeMode,
    ) -> Option<BodyId> {
        if !angle_degrees.is_finite()
            || angle_degrees.abs() < 1.0e-6
            || !axis_origin.is_finite()
            || !axis_direction.is_finite()
            || axis_direction.length_squared() < 1.0e-12
        {
            return None;
        }
        let angle_degrees = angle_degrees.clamp(-360.0, 360.0);
        let SelItem::Profile(history_sketch, history_profile_index) = source else {
            return None;
        };
        let face = match source {
            SelItem::Profile(sketch_id, profile_index) => {
                let sketch = self.sketches.iter().find(|sketch| sketch.id == sketch_id)?;
                let profiles = sketch.profiles();
                sketch.to_face(profiles.get(profile_index)?)?
            }
            _ => return None,
        };
        let axis_direction = axis_direction.normalize();
        let profile = face.into_shape();
        let revolved = profile
            .revolve_face(0, axis_origin, axis_direction, angle_degrees.to_radians())
            .ok()?;
        if !Self::renderable(&revolved) {
            eprintln!("revolve produced no renderable shape");
            return None;
        }
        let support = self
            .sketches
            .iter()
            .find(|sketch| sketch.id == history_sketch)
            .and_then(|sketch| sketch.support_body)
            .filter(|id| self.bodies.iter().any(|body| body.id == *id));
        let target = support.or_else(|| {
            self.bodies.iter().find_map(|body| {
                let common = body.shape.common(&revolved).ok()?;
                Self::renderable(&common).then_some(body.id)
            })
        });
        let resolved = if mode == ExtrudeMode::Auto {
            if support.is_some() {
                ExtrudeMode::Subtract
            } else if target.is_some() {
                ExtrudeMode::Union
            } else {
                ExtrudeMode::NewBody
            }
        } else {
            mode
        };
        let replacement = match (resolved, target) {
            (ExtrudeMode::Union, Some(id)) => Some(
                self.bodies
                    .iter()
                    .find(|body| body.id == id)?
                    .shape
                    .fuse(&revolved)
                    .ok()?,
            ),
            (ExtrudeMode::Subtract, Some(id)) => Some(
                self.bodies
                    .iter()
                    .find(|body| body.id == id)?
                    .shape
                    .cut(&revolved)
                    .ok()?,
            ),
            (ExtrudeMode::Intersect, Some(id)) => Some(
                self.bodies
                    .iter()
                    .find(|body| body.id == id)?
                    .shape
                    .common(&revolved)
                    .ok()?,
            ),
            (ExtrudeMode::NewBody, _) => None,
            _ => return None,
        };
        if replacement
            .as_ref()
            .is_some_and(|shape| !Self::renderable(shape))
        {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = if let (Some(target), Some(replacement)) = (target, replacement) {
            self.bodies.iter_mut().find(|body| body.id == target)?.shape = Arc::new(replacement);
            target
        } else {
            let id = BodyId(self.next_id);
            self.next_id += 1;
            let name = self.next_name("Revolve");
            self.bodies.push(Body {
                id,
                name,
                kind: Self::shape_kind(&revolved),
                shape: Arc::new(revolved),
                visible: true,
                material: Material::default(),
                cosmetic_threads: Vec::new(),
                pose: Mat4::IDENTITY,
            });
            id
        };
        self.selection.items = vec![SelItem::Body(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Revolve {
            sketch: history_sketch,
            profile_index: history_profile_index,
            axis_origin,
            axis_direction,
            angle_degrees: angle_degrees.into(),
            mode,
        });
        Some(id)
    }

    /// Cuts a through or blind parametric hole normal to a planar face.
    pub fn apply_hole(
        &mut self,
        body_id: BodyId,
        face_index: u32,
        at: DVec3,
        diameter: f64,
        kind: HoleKind,
        cut: HoleCut,
    ) -> bool {
        if !at.is_finite() || !diameter.is_finite() || diameter <= 1.0e-6 {
            return false;
        }
        let Some(body) = self.bodies.iter().find(|body| body.id == body_id) else {
            return false;
        };
        let face_reference = face_ref(&body.shape, face_index);
        let Some((_, normal)) = crate::tools::extrude::face_frame(&body.shape, face_index) else {
            return false;
        };
        let Ok((minimum, maximum)) = body.shape.aabb() else {
            return false;
        };
        let diagonal = (maximum - minimum).length().max(1.0);
        let epsilon = diagonal * 1.0e-4;
        let inward = -normal;
        let through_depth = [minimum.x, maximum.x]
            .into_iter()
            .flat_map(|x| {
                [minimum.y, maximum.y].into_iter().flat_map(move |y| {
                    [minimum.z, maximum.z]
                        .into_iter()
                        .map(move |z| (DVec3::new(x, y, z) - at).dot(inward))
                })
            })
            .fold(0.0_f64, f64::max)
            + epsilon * 2.0;
        let depth = match &kind {
            HoleKind::Through => through_depth,
            HoleKind::Blind { depth } if depth.value.is_finite() && depth.value > 1.0e-6 => {
                depth.value
            }
            HoleKind::Blind { .. } => return false,
        };
        let origin = at + normal * epsilon;
        let Ok(mut cutter) = Shape::cylinder(origin, diameter * 0.5, inward, depth + epsilon)
        else {
            return false;
        };
        let entrance = match &cut {
            HoleCut::None => None,
            HoleCut::Counterbore {
                diameter: cb_diameter,
                depth: cb_depth,
            } if cb_diameter.value.is_finite()
                && cb_depth.value.is_finite()
                && cb_diameter.value > diameter
                && cb_depth.value > 1.0e-6 =>
            {
                Shape::cylinder(
                    origin,
                    cb_diameter.value * 0.5,
                    inward,
                    cb_depth.value + epsilon,
                )
                .ok()
            }
            HoleCut::Countersink {
                diameter: cs_diameter,
                angle_degrees,
            } if cs_diameter.value.is_finite()
                && angle_degrees.value.is_finite()
                && cs_diameter.value > diameter
                && angle_degrees.value > 1.0
                && angle_degrees.value < 179.0 =>
            {
                let sink_depth = ((cs_diameter.value - diameter) * 0.5)
                    / (angle_degrees.value.to_radians() * 0.5).tan();
                Shape::cone_axis(
                    origin,
                    cs_diameter.value * 0.5,
                    diameter * 0.5,
                    inward,
                    sink_depth + epsilon,
                )
                .ok()
            }
            _ => return false,
        };
        if let Some(entrance) = entrance {
            let Ok(fused) = cutter.fuse(&entrance) else {
                return false;
            };
            cutter = fused;
        }
        let Ok(result) = body.shape.cut(&cutter) else {
            return false;
        };
        if !Self::renderable(&result) {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies
            .iter_mut()
            .find(|body| body.id == body_id)
            .expect("validated body")
            .shape = Arc::new(result);
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Hole {
            body: body_id,
            face_index: face_reference,
            at,
            diameter: diameter.into(),
            kind,
            cut,
        });
        true
    }

    /// Applies a neutral-plane draft to faces on one body.
    pub fn apply_draft(
        &mut self,
        body_id: BodyId,
        face_indices: &[u32],
        direction: DVec3,
        neutral_origin: DVec3,
        neutral_normal: DVec3,
        angle_degrees: f64,
    ) -> bool {
        if face_indices.is_empty()
            || !direction.is_finite()
            || !neutral_origin.is_finite()
            || !neutral_normal.is_finite()
            || direction.length_squared() < 1.0e-12
            || neutral_normal.length_squared() < 1.0e-12
            || !angle_degrees.is_finite()
            || angle_degrees.abs() < 1.0e-6
            || angle_degrees.abs() >= 89.0
        {
            return false;
        }
        let Some(body) = self.bodies.iter().find(|body| body.id == body_id) else {
            return false;
        };
        let face_references = face_indices
            .iter()
            .map(|&index| face_ref(&body.shape, index))
            .collect();
        let Ok(result) = body.shape.draft_faces(
            face_indices,
            direction.normalize(),
            neutral_origin,
            neutral_normal.normalize(),
            angle_degrees.to_radians(),
        ) else {
            return false;
        };
        if !Self::renderable(&result) {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies
            .iter_mut()
            .find(|body| body.id == body_id)
            .expect("validated body")
            .shape = Arc::new(result);
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Draft {
            body: body_id,
            face_indices: face_references,
            direction: direction.normalize(),
            neutral_origin,
            neutral_normal: neutral_normal.normalize(),
            angle_degrees: angle_degrees.into(),
        });
        true
    }

    /// Sweeps one closed sketch profile along a closed profile or open line chain.
    pub fn apply_sweep(&mut self, section: (SketchId, usize), path: PathRef) -> Option<BodyId> {
        let section_sketch = self.sketches.iter().find(|sketch| sketch.id == section.0)?;
        let section_profile = section_sketch.profiles().get(section.1)?.clone();
        let face = section_sketch.to_face(&section_profile)?;
        let path_wire = match &path {
            PathRef::Profile {
                sketch,
                profile_index,
            } => {
                let path_sketch = self.sketches.iter().find(|item| item.id == *sketch)?;
                let profiles = path_sketch.profiles();
                let profile = profiles.get(*profile_index)?;
                path_sketch.to_wire(profile)?
            }
            PathRef::OpenChain {
                sketch,
                entity_indices,
            } => self
                .sketches
                .iter()
                .find(|item| item.id == *sketch)?
                .open_chain_wire(entity_indices)?,
        };
        let profile = face.into_shape();
        let spine = path_wire.into_shape();
        let result = profile
            .sweep_along(&spine)
            .map_err(|error| {
                eprintln!("sweep failed: {error}");
            })
            .ok()?;
        if !Self::renderable(&result) {
            eprintln!("sweep produced no renderable solid");
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = self.add_body_raw(self.next_name("Sweep"), result);
        self.selection.items = vec![SelItem::Body(id)];
        self.record(HistoryOp::Sweep {
            sketch: section.0,
            profile_index: section.1,
            path,
        });
        Some(id)
    }

    /// Lofts a solid through two or more closed sketch profiles in order.
    pub fn apply_loft(&mut self, sections: &[(SketchId, usize)]) -> Option<BodyId> {
        if sections.len() < 2 {
            return None;
        }
        let wires = sections
            .iter()
            .map(|(sketch_id, profile_index)| {
                let sketch = self
                    .sketches
                    .iter()
                    .find(|sketch| sketch.id == *sketch_id)?;
                let profiles = sketch.profiles();
                sketch.to_wire(profiles.get(*profile_index)?)
            })
            .collect::<Option<Vec<_>>>()?;
        let wires = wires.into_iter().map(occt::Wire::into_shape).collect();
        let result = Shape::loft(wires)
            .map_err(|error| {
                eprintln!("loft failed: {error}");
            })
            .ok()?;
        if !Self::renderable(&result) {
            eprintln!("loft produced no renderable solid");
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = self.add_body_raw(self.next_name("Loft"), result);
        self.selection.items = vec![SelItem::Body(id)];
        self.record(HistoryOp::Loft {
            sections: sections.to_vec(),
        });
        Some(id)
    }

    /// Creates a solid spring by sweeping a circular profile along an exact helix wire.
    pub fn apply_helix(
        &mut self,
        origin: DVec3,
        axis: DVec3,
        radius: f64,
        pitch: f64,
        turns: f64,
        profile_radius: f64,
        left_handed: bool,
    ) -> Option<BodyId> {
        if !origin.is_finite()
            || !axis.is_finite()
            || axis.length_squared() < 1.0e-12
            || !radius.is_finite()
            || !pitch.is_finite()
            || !turns.is_finite()
            || !profile_radius.is_finite()
            || radius <= 1.0e-6
            || pitch <= 1.0e-6
            || turns <= 1.0e-6
            || profile_radius <= 1.0e-6
        {
            return None;
        }
        let axis = axis.normalize();
        let spine = Shape::helix_wire(origin, axis, radius, pitch, turns, left_handed).ok()?;
        let start = spine.edge_start_point(0).ok()?;
        let axial = axis * (start - origin).dot(axis);
        let radial = (start - origin - axial).normalize_or_zero();
        let handed = if left_handed { -1.0 } else { 1.0 };
        let tangent = (axis.cross(radial) * (handed * radius)
            + axis * (pitch / std::f64::consts::TAU))
            .normalize_or_zero();
        let circle = occt::Edge::circle(start, tangent, profile_radius).ok()?;
        let wire = occt::Wire::from_edges(vec![circle]).ok()?;
        let profile = occt::Face::from_wire(&wire).ok()?.into_shape();
        let result = profile.sweep_along(&spine).ok()?;
        if !Self::renderable(&result) {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = self.add_body_raw(self.next_name("Helix"), result);
        self.selection.items = vec![SelItem::Body(id)];
        self.record(HistoryOp::Helix {
            origin,
            axis,
            radius: radius.into(),
            pitch: pitch.into(),
            turns: turns.into(),
            profile_radius: profile_radius.into(),
            left_handed,
        });
        Some(id)
    }

    /// Adds copies of bodies reflected across an explicit world-space plane.
    pub fn apply_mirror(
        &mut self,
        ids: &[BodyId],
        plane_origin: DVec3,
        plane_normal: DVec3,
    ) -> Vec<BodyId> {
        if !plane_origin.is_finite()
            || !plane_normal.is_finite()
            || plane_normal.length_squared() < 1.0e-12
        {
            return Vec::new();
        }
        self.prepare_non_sketch_op();
        let plane_normal = plane_normal.normalize();
        let sources = self.unique_bodies(ids);
        let source_ids: Vec<_> = sources.iter().map(|body| body.id).collect();
        let copies: Vec<_> = sources
            .iter()
            .map(|body| {
                let shape = body.shape.mirrored(plane_origin, plane_normal).ok()?;
                let stem = format!("{} Mirror", body.name);
                let name = if self.bodies.iter().all(|candidate| candidate.name != stem) {
                    stem
                } else {
                    self.next_name(&stem)
                };
                Some((name, shape))
            })
            .collect::<Option<Vec<_>>>()
            .unwrap_or_default();
        let copies = self.add_transformed_copies(copies, &[]);
        if !copies.is_empty() {
            self.record(HistoryOp::Mirror {
                ids: source_ids,
                plane_origin,
                plane_normal,
            });
        }
        copies
    }

    /// Adds `count - 1` copies at equal spacing along a world-space direction.
    pub fn apply_linear_pattern(
        &mut self,
        ids: &[BodyId],
        axis_origin: DVec3,
        axis_direction: DVec3,
        count: usize,
        spacing: f64,
    ) -> Vec<BodyId> {
        if !(2..=12).contains(&count)
            || !spacing.is_finite()
            || spacing.abs() < 1.0e-6
            || !axis_origin.is_finite()
            || !axis_direction.is_finite()
            || axis_direction.length_squared() < 1.0e-12
        {
            return Vec::new();
        }
        self.prepare_non_sketch_op();
        let axis_direction = axis_direction.normalize();
        let sources = self.unique_bodies(ids);
        let source_ids: Vec<_> = sources.iter().map(|body| body.id).collect();
        let copies = (1..count)
            .flat_map(|instance| {
                sources.iter().map(move |body| {
                    (
                        format!("{} {}", body.name, instance + 1),
                        body.shape
                            .translated(axis_direction * spacing * instance as f64)
                            .ok(),
                    )
                })
            })
            .filter_map(|(name, shape)| Some((name, shape?)))
            .collect();
        let copies = self.add_transformed_copies(copies, ids);
        if !copies.is_empty() {
            self.record(HistoryOp::LinearPattern {
                ids: source_ids,
                axis_origin,
                axis_direction,
                spacing: spacing.into(),
                count: count as u32,
            });
        }
        copies
    }

    /// Adds `count - 1` rotations over one full circle around a world-space axis.
    pub fn apply_circular_pattern(
        &mut self,
        ids: &[BodyId],
        axis_origin: DVec3,
        axis_direction: DVec3,
        count: usize,
    ) -> Vec<BodyId> {
        if !(2..=24).contains(&count)
            || !axis_origin.is_finite()
            || !axis_direction.is_finite()
            || axis_direction.length_squared() < 1.0e-12
        {
            return Vec::new();
        }
        self.prepare_non_sketch_op();
        let axis_direction = axis_direction.normalize();
        let sources = self.unique_bodies(ids);
        let source_ids: Vec<_> = sources.iter().map(|body| body.id).collect();
        let step = std::f64::consts::TAU / count as f64;
        let copies = (1..count)
            .flat_map(|instance| {
                sources.iter().map(move |body| {
                    (
                        format!("{} {}", body.name, instance + 1),
                        body.shape
                            .rotated(axis_origin, axis_direction, step * instance as f64)
                            .ok(),
                    )
                })
            })
            .filter_map(|(name, shape)| Some((name, shape?)))
            .collect();
        let copies = self.add_transformed_copies(copies, ids);
        if !copies.is_empty() {
            self.record(HistoryOp::CircularPattern {
                ids: source_ids,
                axis_origin,
                axis_direction,
                count: count as u32,
            });
        }
        copies
    }

    fn unique_bodies(&self, ids: &[BodyId]) -> Vec<&Body> {
        self.bodies
            .iter()
            .filter(|body| ids.contains(&body.id))
            .collect()
    }

    fn add_transformed_copies(
        &mut self,
        copies: Vec<(String, Shape)>,
        originals: &[BodyId],
    ) -> Vec<BodyId> {
        if copies.is_empty() || copies.iter().any(|(_, shape)| !Self::renderable(shape)) {
            return Vec::new();
        }
        self.push_undo();
        let mut new_ids = Vec::with_capacity(copies.len());
        for (index, (name, shape)) in copies.into_iter().enumerate() {
            let id = BodyId(self.next_id);
            self.next_id += 1;
            let material = originals
                .get(index % originals.len().max(1))
                .and_then(|id| self.bodies.iter().find(|body| body.id == *id))
                .map_or_else(Material::default, |body| body.material);
            self.bodies.push(Body {
                id,
                name,
                kind: Self::shape_kind(&shape),
                shape: Arc::new(shape),
                visible: true,
                material,
                cosmetic_threads: Vec::new(),
                pose: Mat4::IDENTITY,
            });
            new_ids.push(id);
        }
        self.selection.items = originals
            .iter()
            .copied()
            .chain(new_ids.iter().copied())
            .map(SelItem::Body)
            .collect();
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        new_ids
    }

    /// Applies one face extrusion as a single undo step.
    pub fn apply_extrude(&mut self, drag: &ExtrudeDrag) -> bool {
        let face_reference = self
            .bodies
            .iter()
            .find(|body| body.id == drag.body)
            .map(|body| face_ref(&body.shape, drag.face_index));
        self.prepare_non_sketch_op();
        let committed = crate::tools::extrude::commit(self, drag);
        if committed {
            self.record(HistoryOp::Extrude {
                sketch: None,
                profile_index: None,
                body: Some(drag.body),
                face_index: face_reference,
                distance: drag.distance.into(),
                opposite_distance: drag.opposite_distance.into(),
                side_mode: drag.side_mode,
                mode: drag.mode,
            });
        }
        committed
    }

    /// Applies one planar-face offset as a single undo step in Auto mode.
    pub fn apply_offset_face(&mut self, drag: &ExtrudeDrag) -> bool {
        let face_reference = self
            .bodies
            .iter()
            .find(|body| body.id == drag.body)
            .map(|body| face_ref(&body.shape, drag.face_index));
        self.prepare_non_sketch_op();
        let committed = crate::tools::offset_face::commit(self, drag);
        if committed {
            self.record(HistoryOp::OffsetFace {
                body: drag.body,
                face_index: face_reference.expect("committed offset source face"),
                distance: drag.distance.into(),
            });
        }
        committed
    }

    /// Replaces a planar face with a parallel target plane.
    ///
    /// The selected face's outward normal defines the kept-material side. A target
    /// outside the body fuses the prism between the old and new planes (extend); a
    /// target inside cuts that prism away (trim). Non-parallel targets are rejected.
    pub fn apply_replace_face(
        &mut self,
        body_id: BodyId,
        face_index: u32,
        target_origin: DVec3,
        target_normal: DVec3,
    ) -> bool {
        if !target_origin.is_finite()
            || !target_normal.is_finite()
            || target_normal.length_squared() < 1.0e-12
        {
            return false;
        }
        let Some(body) = self.bodies.iter().find(|body| body.id == body_id) else {
            return false;
        };
        let face_reference = face_ref(&body.shape, face_index);
        let Some((face_origin, face_normal)) =
            crate::tools::extrude::face_frame(&body.shape, face_index)
        else {
            return false;
        };
        if face_normal.dot(target_normal.normalize()).abs() < 1.0 - 1.0e-6 {
            return false;
        }
        let distance = (target_origin - face_origin).dot(face_normal);
        if distance.abs() <= 1.0e-8 {
            return false;
        }
        let Ok(prism) = body
            .shape
            .extrude_face(face_index as usize, face_normal * distance)
        else {
            return false;
        };
        let result = if distance > 0.0 {
            body.shape.fuse(&prism)
        } else {
            body.shape.cut(&prism)
        };
        let Ok(result) = result else {
            return false;
        };
        if !Self::renderable(&result) {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies
            .iter_mut()
            .find(|body| body.id == body_id)
            .expect("validated body")
            .shape = Arc::new(result);
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::ReplaceFace {
            body: body_id,
            face_index: face_reference,
            target_origin,
            target_normal: target_normal.normalize(),
        });
        true
    }

    /// Applies one sketch-profile extrusion as a single undo step.
    pub fn apply_profile_extrude(&mut self, drag: &ProfileExtrudeDrag) -> bool {
        self.prepare_non_sketch_op();
        let committed = crate::tools::extrude::commit_profile(self, drag);
        if committed {
            self.record(HistoryOp::Extrude {
                sketch: Some(drag.sketch),
                profile_index: Some(drag.profile_index),
                body: None,
                face_index: None,
                distance: drag.distance.into(),
                opposite_distance: drag.opposite_distance.into(),
                side_mode: drag.side_mode,
                mode: drag.mode,
            });
        }
        committed
    }

    pub(crate) fn apply_profile_extrude_result(
        &mut self,
        target: Option<BodyId>,
        prism: Shape,
        replacement: Option<Shape>,
    ) -> bool {
        self.push_undo();
        if let (Some(target), Some(replacement)) = (target, replacement) {
            let Some(body) = self.bodies.iter_mut().find(|body| body.id == target) else {
                return false;
            };
            body.shape = Arc::new(replacement);
            self.selection.items = vec![SelItem::Body(target)];
        } else {
            let id = BodyId(self.next_id);
            self.next_id += 1;
            let name = self.next_name("Extrude");
            self.bodies.push(Body {
                id,
                name,
                kind: Self::shape_kind(&prism),
                shape: Arc::new(prism),
                visible: true,
                material: Material::default(),
                cosmetic_threads: Vec::new(),
                pose: Mat4::IDENTITY,
            });
            self.selection.items = vec![SelItem::Body(id)];
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    pub(crate) fn apply_extrude_result(
        &mut self,
        body_id: BodyId,
        prism: Shape,
        replacement: Option<Shape>,
    ) -> bool {
        let Some(index) = self.bodies.iter().position(|body| body.id == body_id) else {
            return false;
        };
        let source_material = self.bodies[index].material;
        self.push_undo();
        if let Some(shape) = replacement {
            self.bodies[index].shape = Arc::new(shape);
            self.selection.items = vec![SelItem::Body(body_id)];
        } else {
            let id = BodyId(self.next_id);
            self.next_id += 1;
            let name = self.next_name("Extrude");
            self.bodies.push(Body {
                id,
                name,
                kind: Self::shape_kind(&prism),
                shape: Arc::new(prism),
                visible: true,
                material: source_material,
                cosmetic_threads: Vec::new(),
                pose: Mat4::IDENTITY,
            });
            self.selection.items = vec![SelItem::Body(id)];
        }
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        true
    }

    /// Extrudes a connected open sketch chain into a new surface body.
    pub fn apply_open_chain_extrude(
        &mut self,
        sketch_id: SketchId,
        entity_indices: &[usize],
        distance: f64,
        opposite_distance: f64,
        side_mode: crate::tools::extrude::ExtrudeSideMode,
    ) -> Option<BodyId> {
        let (start, length) =
            crate::tools::extrude::extrusion_extents(distance, opposite_distance, side_mode)?;
        if length.abs() < 1.0e-6 {
            return None;
        }
        let sketch = self.sketches.iter().find(|sketch| sketch.id == sketch_id)?;
        let normal = sketch.plane.normal();
        let wire = sketch.open_chain_wire(entity_indices)?.into_shape();
        let surface = wire
            .prism_of_wire_shape(normal * length)
            .ok()?
            .translated(normal * start)
            .ok()?;
        if !Self::renderable(&surface) || Self::shape_kind(&surface) != BodyKind::Surface {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = BodyId(self.next_id);
        self.next_id += 1;
        let name = self.next_name("Surface Extrude");
        self.bodies.push(Body {
            id,
            name,
            shape: Arc::new(surface),
            kind: BodyKind::Surface,
            visible: true,
            material: Material::default(),
            cosmetic_threads: Vec::new(),
            pose: Mat4::IDENTITY,
        });
        self.selection.items = vec![SelItem::Body(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::SurfaceExtrude {
            sketch: sketch_id,
            entity_indices: entity_indices.to_vec(),
            distance: distance.into(),
            opposite_distance: opposite_distance.into(),
            side_mode,
        });
        Some(id)
    }

    /// Revolves a connected open sketch chain into a new surface body.
    pub fn apply_open_chain_revolve(
        &mut self,
        sketch_id: SketchId,
        entity_indices: &[usize],
        axis_origin: DVec3,
        axis_direction: DVec3,
        angle_degrees: f64,
    ) -> Option<BodyId> {
        if !axis_origin.is_finite()
            || !axis_direction.is_finite()
            || axis_direction.length_squared() < 1.0e-12
            || !angle_degrees.is_finite()
            || angle_degrees.abs() < 1.0e-6
        {
            return None;
        }
        let sketch = self.sketches.iter().find(|sketch| sketch.id == sketch_id)?;
        let wire = sketch.open_chain_wire(entity_indices)?.into_shape();
        let direction = axis_direction.normalize();
        let surface = wire
            .revolve_wire(axis_origin, direction, angle_degrees.to_radians())
            .ok()?;
        if !Self::renderable(&surface) || Self::shape_kind(&surface) != BodyKind::Surface {
            return None;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = BodyId(self.next_id);
        self.next_id += 1;
        let name = self.next_name("Surface Revolve");
        self.bodies.push(Body {
            id,
            name,
            shape: Arc::new(surface),
            kind: BodyKind::Surface,
            visible: true,
            material: Material::default(),
            cosmetic_threads: Vec::new(),
            pose: Mat4::IDENTITY,
        });
        self.selection.items = vec![SelItem::Body(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::SurfaceRevolve {
            sketch: sketch_id,
            entity_indices: entity_indices.to_vec(),
            axis_origin,
            axis_direction: direction,
            angle_degrees: angle_degrees.into(),
        });
        Some(id)
    }

    /// Fills selected boundary edges as a separate surface body.
    pub fn apply_patch(&mut self, body_id: BodyId, edge_indices: &[u32]) -> Option<BodyId> {
        let body = self.bodies.iter().find(|body| body.id == body_id)?;
        let edge_references = edge_indices
            .iter()
            .map(|&index| edge_ref(&body.shape, index))
            .collect();
        let edge_count = body.shape.edge_count().ok()?;
        if edge_indices.is_empty()
            || edge_indices
                .iter()
                .any(|index| *index as usize >= edge_count)
        {
            return None;
        }
        let patch = body.shape.patch_face(edge_indices).ok()?;
        if !Self::renderable(&patch) {
            return None;
        }
        let material = body.material;
        self.prepare_non_sketch_op();
        self.push_undo();
        let id = BodyId(self.next_id);
        self.next_id += 1;
        let name = self.next_name("Patch");
        self.bodies.push(Body {
            id,
            name,
            shape: Arc::new(patch),
            kind: BodyKind::Surface,
            visible: true,
            material,
            cosmetic_threads: Vec::new(),
            pose: Mat4::IDENTITY,
        });
        self.selection.items = vec![SelItem::Body(id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Patch {
            body: body_id,
            edges: edge_references,
        });
        Some(id)
    }

    /// Sews surface bodies and replaces them with one shell or solid.
    pub fn apply_stitch(&mut self, ids: &[BodyId]) -> Option<BodyId> {
        let mut unique = Vec::new();
        for &id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        if unique.len() < 2 {
            return None;
        }
        let sources: Vec<_> = unique
            .iter()
            .map(|id| {
                self.bodies
                    .iter()
                    .find(|body| body.id == *id && body.kind == BodyKind::Surface)
            })
            .collect::<Option<_>>()?;
        let shapes = sources
            .iter()
            .map(|body| body.shape.try_clone().ok())
            .collect::<Option<Vec<_>>>()?;
        let result = Shape::stitch(shapes, 1.0e-4).ok()?;
        if !Self::renderable(&result) {
            return None;
        }
        let kind = Self::shape_kind(&result);
        let target = unique[0];
        let target_index = self.bodies.iter().position(|body| body.id == target)?;
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies[target_index].shape = Arc::new(result);
        self.bodies[target_index].kind = kind;
        self.bodies[target_index].name = self.next_name("Stitch");
        self.bodies
            .retain(|body| body.id == target || !unique[1..].contains(&body.id));
        self.selection.items = vec![SelItem::Body(target)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Stitch { bodies: unique });
        Some(target)
    }

    /// Thickens a surface body into a solid in place.
    pub fn apply_thicken(&mut self, body_id: BodyId, thickness: f64) -> bool {
        if !thickness.is_finite() || thickness.abs() < 1.0e-4 {
            return false;
        }
        let Some(index) = self
            .bodies
            .iter()
            .position(|body| body.id == body_id && body.kind == BodyKind::Surface)
        else {
            return false;
        };
        let Ok(result) = self.bodies[index].shape.thicken(thickness) else {
            return false;
        };
        if !Self::renderable(&result) || Self::shape_kind(&result) != BodyKind::Solid {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies[index].shape = Arc::new(result);
        self.bodies[index].kind = BodyKind::Solid;
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Thicken {
            body: body_id,
            thickness: thickness.into(),
        });
        true
    }

    /// Deletes solid faces and commits only when OCCT heals a valid solid.
    pub fn apply_delete_faces(&mut self, body_id: BodyId, face_indices: &[u32]) -> bool {
        let Some(index) = self
            .bodies
            .iter()
            .position(|body| body.id == body_id && body.kind == BodyKind::Solid)
        else {
            return false;
        };
        let face_count = self.bodies[index].shape.face_count().ok().unwrap_or(0);
        if face_indices.is_empty() || face_indices.iter().any(|face| *face as usize >= face_count) {
            return false;
        }
        let face_references = face_indices
            .iter()
            .map(|&face| face_ref(&self.bodies[index].shape, face))
            .collect();
        let Ok(result) = self.bodies[index].shape.delete_faces(face_indices) else {
            return false;
        };
        if !Self::renderable(&result) || Self::shape_kind(&result) != BodyKind::Solid {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies[index].shape = Arc::new(result);
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::DeleteFace {
            body: body_id,
            faces: face_references,
        });
        true
    }

    fn renderable(shape: &Shape) -> bool {
        shape.face_count().is_ok_and(|count| count > 0) && shape.mesh(0.5).is_ok()
    }

    fn shape_kind(shape: &Shape) -> BodyKind {
        if shape.solid_count().is_ok_and(|count| count > 0) {
            BodyKind::Solid
        } else {
            BodyKind::Surface
        }
    }

    /// Applies a boolean to the first id and removes every remaining tool body.
    pub fn apply_boolean(&mut self, op: BooleanOp, ids: &[BodyId]) -> bool {
        let mut unique = Vec::new();
        for &id in ids {
            if !unique.contains(&id) {
                unique.push(id);
            }
        }
        if unique.len() < 2
            || unique.iter().any(|id| {
                !self
                    .bodies
                    .iter()
                    .any(|body| body.id == *id && body.kind == BodyKind::Solid)
            })
        {
            return false;
        }
        let target = unique[0];
        let target_shape = self
            .bodies
            .iter()
            .find(|body| body.id == target)
            .map(|body| body.shape.as_ref())
            .expect("validated boolean target");
        let mut tools = unique.iter().skip(1).filter_map(|id| {
            self.bodies
                .iter()
                .find(|body| body.id == *id)
                .map(|body| body.shape.as_ref())
        });
        let first_tool = tools.next().expect("at least one validated boolean tool");
        let operation = |left: &Shape, right: &Shape| match op {
            BooleanOp::Union => left.fuse(right),
            BooleanOp::Subtract => left.cut(right),
            BooleanOp::Intersect => left.common(right),
        };
        let Ok(mut result) = operation(target_shape, first_tool) else {
            return false;
        };
        if !Self::renderable(&result) {
            eprintln!("boolean operation produced no renderable shape");
            return false;
        }
        for tool in tools {
            let Ok(next) = operation(&result, tool) else {
                return false;
            };
            result = next;
            if !Self::renderable(&result) {
                eprintln!("boolean operation produced no renderable shape");
                return false;
            }
        }
        let target_index = self
            .bodies
            .iter()
            .position(|body| body.id == target)
            .expect("validated boolean target");
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies[target_index].shape = Arc::new(result);
        self.bodies
            .retain(|body| body.id == target || !unique[1..].contains(&body.id));
        self.selection.items = vec![SelItem::Body(target)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Boolean {
            op,
            target,
            tools: unique[1..].to_vec(),
        });
        true
    }

    /// Applies a fillet or chamfer to selected edges as one undo step.
    pub fn apply_dressup(&mut self, body_id: BodyId, operation: DressUp) -> bool {
        let Some(body) = self
            .bodies
            .iter()
            .find(|body| body.id == body_id && body.kind == BodyKind::Solid)
        else {
            return false;
        };
        let (radius, edge_indices, fillet) = match &operation {
            DressUp::Fillet {
                radius,
                edge_indices,
                ..
            } => (*radius, edge_indices, true),
            DressUp::Chamfer {
                radius,
                edge_indices,
            } => (*radius, edge_indices, false),
        };
        if radius < 0.01 || edge_indices.is_empty() {
            return false;
        }
        if edge_indices
            .iter()
            .any(|&index| index as usize >= body.shape.edge_count().ok().unwrap_or(0))
        {
            return false;
        }
        let edge_references = edge_indices
            .iter()
            .map(|&edge| edge_ref(&body.shape, edge))
            .collect();
        let end_radius = match &operation {
            DressUp::Fillet { end_radius, .. } => *end_radius,
            DressUp::Chamfer { .. } => None,
        };
        let result = if fillet {
            if let Some(end) = end_radius {
                body.shape.variable_fillet_edges(edge_indices, radius, end)
            } else {
                body.shape.fillet_edges(radius, edge_indices)
            }
        } else {
            body.shape.chamfer_edges(radius, edge_indices)
        };
        let Ok(result) = result else { return false };
        if !Self::renderable(&result) {
            eprintln!("edge dress-up produced no renderable shape");
            return false;
        }
        let index = self
            .bodies
            .iter()
            .position(|body| body.id == body_id)
            .expect("dress-up body still exists");
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies[index].shape = Arc::new(result);
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(if fillet {
            HistoryOp::Fillet {
                body: body_id,
                edges: edge_references,
                radius: radius.into(),
                end_radius: end_radius.map(Into::into),
            }
        } else {
            HistoryOp::Chamfer {
                body: body_id,
                edges: edge_references,
                radius: radius.into(),
            }
        });
        true
    }

    /// Adds a cosmetic thread or cuts a swept triangular helical groove.
    pub fn apply_thread(
        &mut self,
        body_id: BodyId,
        face_index: u32,
        external: bool,
        mode: ThreadMode,
        pitch: f64,
        depth: f64,
    ) -> bool {
        let Some(index) = self.bodies.iter().position(|body| body.id == body_id) else {
            return false;
        };
        let face_reference = face_ref(&self.bodies[index].shape, face_index);
        let Some((origin, axis, radius, height)) = self.bodies[index]
            .shape
            .face_cylinder_data(face_index as usize)
            .ok()
        else {
            return false;
        };
        let depth = depth.min(height);
        if pitch <= 1.0e-6 || depth <= 1.0e-6 {
            return false;
        }
        self.prepare_non_sketch_op();
        self.push_undo();
        if mode == ThreadMode::Cosmetic {
            self.bodies[index].cosmetic_threads.push(CosmeticThread {
                face_index,
                external,
                origin,
                axis,
                radius,
                pitch,
                depth,
            });
        } else {
            let turns = depth / pitch;
            let spine = match Shape::helix_wire(origin, axis, radius, pitch, turns, false) {
                Ok(shape) => shape,
                Err(_) => {
                    self.undo.pop();
                    return false;
                }
            };
            let Some(start) = spine.edge_start_point(0).ok() else {
                self.undo.pop();
                return false;
            };
            let radial = (start - origin - axis * (start - origin).dot(axis)).normalize_or_zero();
            let groove_depth = (pitch * 0.35).min(radius * 0.25);
            let material_direction = if external { -radial } else { radial };
            let half_width = pitch * 0.28;
            let points = [
                start - axis * half_width,
                start + material_direction * groove_depth,
                start + axis * half_width,
            ];
            let edges = [
                occt::Edge::segment(points[0], points[1]),
                occt::Edge::segment(points[1], points[2]),
                occt::Edge::segment(points[2], points[0]),
            ]
            .into_iter()
            .collect::<Result<Vec<_>, _>>();
            let result = edges
                .and_then(occt::Wire::from_edges)
                .and_then(|wire| occt::Face::from_wire(&wire))
                .map(occt::Face::into_shape)
                .and_then(|profile| profile.sweep_along(&spine))
                .and_then(|tool| self.bodies[index].shape.cut(&tool));
            let Ok(result) = result else {
                self.undo.pop();
                return false;
            };
            if !Self::renderable(&result) {
                self.undo.pop();
                return false;
            }
            self.bodies[index].shape = Arc::new(result);
        }
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Thread {
            body: body_id,
            face_index: face_reference,
            external,
            mode,
            pitch: pitch.into(),
            depth: depth.into(),
        });
        true
    }

    /// Hollows a body inward and removes the selected opening faces.
    pub fn apply_shell(&mut self, body_id: BodyId, face_indices: &[u32], thickness: f64) -> bool {
        let Some(body) = self
            .bodies
            .iter()
            .find(|body| body.id == body_id && body.kind == BodyKind::Solid)
        else {
            return false;
        };
        if thickness < 0.05 || face_indices.is_empty() {
            return false;
        }
        if face_indices
            .iter()
            .any(|&index| index as usize >= body.shape.face_count().ok().unwrap_or(0))
        {
            return false;
        }
        let face_references = face_indices
            .iter()
            .map(|&face| face_ref(&body.shape, face))
            .collect();
        let Ok(result) = body.shape.hollow(face_indices, -thickness) else {
            return false;
        };
        if !Self::renderable(&result) {
            eprintln!("shell operation produced no renderable shape");
            return false;
        }
        let index = self
            .bodies
            .iter()
            .position(|body| body.id == body_id)
            .expect("shell body still exists");
        self.prepare_non_sketch_op();
        self.push_undo();
        self.bodies[index].shape = Arc::new(result);
        self.selection.items = vec![SelItem::Body(body_id)];
        self.scene_epoch = self.scene_epoch.wrapping_add(1);
        self.record(HistoryOp::Shell {
            body: body_id,
            faces: face_references,
            thickness: thickness.into(),
        });
        true
    }

    /// Restores the previous document snapshot.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        if self.redo.len() == MAX_SNAPSHOTS {
            self.redo.remove(0);
        }
        self.redo.push(self.snapshot());
        self.restore(previous);
        true
    }

    /// Restores the next snapshot from redo history.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        if self.undo.len() == MAX_SNAPSHOTS {
            self.undo.remove(0);
        }
        self.undo.push(self.snapshot());
        self.restore(next);
        true
    }

    /// Removes selected items whose owning body or profile no longer exists.
    pub fn sanitize_selection(&mut self) {
        self.selection.items.retain(|item| match *item {
            SelItem::Plane(id) => self.construction_planes.iter().any(|plane| plane.id == id),
            SelItem::Axis(id) => self.construction_axes.iter().any(|axis| axis.id == id),
            SelItem::Point(id) => self.construction_points.iter().any(|point| point.id == id),
            SelItem::Profile(id, index) => self
                .sketches
                .iter()
                .find(|sketch| sketch.id == id)
                .is_some_and(|sketch| index < sketch.profiles().len()),
            SelItem::SketchEntity(id, index) => self
                .sketches
                .iter()
                .find(|sketch| sketch.id == id)
                .is_some_and(|sketch| index < sketch.entities.len()),
            _ => item
                .body_id()
                .is_some_and(|id| self.bodies.iter().any(|body| body.id == id)),
        });
    }

    /// Returns an unused auto-name such as `Box 3`.
    pub fn next_name(&self, stem: &str) -> String {
        let mut number = 1;
        loop {
            let candidate = format!("{stem} {number}");
            if self.bodies.iter().all(|body| body.name != candidate) {
                return candidate;
            }
            number += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::{EntityRef, PointRef};
    use glam::DVec3;

    struct TestBounds {
        minimum: DVec3,
        maximum: DVec3,
    }

    impl TestBounds {
        fn min(&self) -> DVec3 {
            self.minimum
        }
        fn max(&self) -> DVec3 {
            self.maximum
        }
    }

    fn aabb(shape: &Shape) -> TestBounds {
        let (minimum, maximum) = shape.aabb().expect("test shape bounds");
        TestBounds { minimum, maximum }
    }

    fn variable_extrude_document() -> (Document, usize) {
        let kind = PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        };
        let shape = kind.shape();
        let face_index = (0..shape.face_count().unwrap())
            .filter_map(|index| {
                crate::tools::extrude::face_frame(&shape, index as u32)
                    .map(|(origin, _)| (index as u32, origin.z))
            })
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .unwrap()
            .0;
        let steps = vec![
            HistoryStep::new(HistoryOp::AddPrimitive { kind }),
            HistoryStep::new(HistoryOp::Extrude {
                sketch: None,
                profile_index: None,
                body: Some(BodyId(1)),
                face_index: Some(face_index.into()),
                distance: crate::history::Num::from_input(5.0, "height".to_owned()),
                opposite_distance: 0.0.into(),
                side_mode: crate::tools::extrude::ExtrudeSideMode::OneSided,
                mode: ExtrudeMode::Auto,
            }),
        ];
        let mut document = crate::history::replay(&steps).unwrap();
        document.variables.push(Variable {
            name: "height".to_owned(),
            expr: "5".to_owned(),
            value: 5.0,
            error: None,
        });
        (document, 0)
    }

    #[test]
    fn variable_edit_reevaluates_extrude_and_changes_bounds() {
        let (mut document, variable) = variable_extrude_document();
        let before = aabb(&document.bodies[0].shape).max().z;
        assert!(document.update_variable(variable, "height".to_owned(), "12".to_owned()));
        let after = aabb(&document.bodies[0].shape).max().z;
        assert!(after > before + 6.9, "before={before}, after={after}");
        let HistoryOp::Extrude { distance, .. } = &document.history[1].op else {
            panic!("expected extrude")
        };
        assert_eq!(distance.value, 12.0);
        assert_eq!(distance.expr.as_deref(), Some("height"));
    }

    #[test]
    fn failed_feature_reevaluation_keeps_geometry_and_flags_variable() {
        let (mut document, variable) = variable_extrude_document();
        let before = aabb(&document.bodies[0].shape).max().z;
        assert!(document.update_variable(variable, "height".to_owned(), "0".to_owned()));
        let after = aabb(&document.bodies[0].shape).max().z;
        assert_eq!(after, before);
        assert!(
            document.variables[variable]
                .error
                .as_deref()
                .is_some_and(|error| error.contains(crate::i18n::t("recompute failed")))
        );
        let HistoryOp::Extrude { distance, .. } = &document.history[1].op else {
            panic!("expected extrude")
        };
        assert_eq!(distance.value, 5.0, "failed replay must keep old history");
    }

    #[test]
    fn construction_plane_from_face_offsets_along_normal() {
        let shape = Shape::box_from_corners(DVec3::ZERO, DVec3::splat(10.0)).unwrap();
        let (face_index, base) = (0..shape.face_count().unwrap())
            .filter_map(|index| {
                SketchPlane::from_face(&shape, index as u32).map(|plane| (index as u32, plane))
            })
            .max_by(|(_, left), (_, right)| left.origin.z.total_cmp(&right.origin.z))
            .expect("planar box face");
        assert!(SketchPlane::from_face(&shape, face_index).is_some());
        let mut document = Document::new();
        let id = document.add_offset_construction_plane(base, 7.5);
        let plane = document
            .construction_planes
            .iter()
            .find(|plane| plane.id == id)
            .expect("construction plane");
        assert!((plane.plane.origin - (base.origin + base.normal() * 7.5)).length() < 1.0e-9);
    }

    fn box_shape() -> Shape {
        Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap()
    }

    fn add_variable_rectangle(document: &mut Document) -> SketchId {
        let id = document.add_sketch(SketchPlane::xy());
        let points = [
            DVec2::ZERO,
            DVec2::new(40.0, 0.0),
            DVec2::new(40.0, 30.0),
            DVec2::new(0.0, 30.0),
        ];
        let mut constraints = (0..4)
            .map(|index| Constraint::Coincident {
                a: PointRef {
                    entity: index,
                    point: 1,
                },
                b: PointRef {
                    entity: (index + 1) % 4,
                    point: 0,
                },
            })
            .collect::<Vec<_>>();
        constraints.extend([
            Constraint::Horizontal(EntityRef(0)),
            Constraint::Vertical(EntityRef(1)),
            Constraint::Horizontal(EntityRef(2)),
            Constraint::Vertical(EntityRef(3)),
            Constraint::Length {
                line: EntityRef(0),
                value: 40.0,
                expr: Some("w".to_owned()),
                error: None,
                reference: false,
            },
            Constraint::Distance {
                a: PointRef {
                    entity: 0,
                    point: 0,
                },
                b: PointRef {
                    entity: 3,
                    point: 0,
                },
                value: 30.0,
                expr: None,
                error: None,
                reference: false,
            },
        ]);
        assert!(document.add_sketch_entities_with_constraints(
            id,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            }),
            constraints,
        ));
        id
    }

    fn profile_width(document: &Document, id: SketchId) -> f64 {
        let profile = document
            .sketches
            .iter()
            .find(|sketch| sketch.id == id)
            .expect("variable rectangle")
            .profiles()
            .into_iter()
            .next()
            .expect("closed rectangle profile");
        let crate::sketch::Profile::LineLoop(points) = profile else {
            panic!("rectangle must produce a line-loop profile")
        };
        let minimum = points
            .iter()
            .map(|point| point.x)
            .fold(f64::INFINITY, f64::min);
        let maximum = points
            .iter()
            .map(|point| point.x)
            .fold(f64::NEG_INFINITY, f64::max);
        maximum - minimum
    }

    #[test]
    fn variables_chain_in_order_and_invalid_expression_keeps_last_value() {
        let mut document = Document::new();
        let a = document.add_variable();
        assert!(document.update_variable(a, "a".to_owned(), "20".to_owned()));
        let b = document.add_variable();
        assert!(document.update_variable(b, "b".to_owned(), "a * 2".to_owned()));
        assert_eq!(document.variables[b].value, 40.0);
        assert!(document.update_variable(b, "b".to_owned(), "missing + 1".to_owned()));
        assert_eq!(document.variables[b].value, 40.0);
        assert_eq!(
            document.variables[b].error.as_deref(),
            Some(crate::i18n::t("Undefined variable missing"))
        );
    }

    #[test]
    fn variable_edit_resolves_rectangle_profile_and_is_undoable() {
        let mut document = Document::new();
        let variable = document.add_variable();
        assert!(document.update_variable(variable, "w".to_owned(), "40".to_owned()));
        let sketch = add_variable_rectangle(&mut document);
        assert!((profile_width(&document, sketch) - 40.0).abs() < 1.0e-6);
        let revision = document.revision;
        assert!(document.update_variable(variable, "w".to_owned(), "60".to_owned()));
        assert_ne!(document.revision, revision);
        assert!((profile_width(&document, sketch) - 60.0).abs() < 1.0e-6);
        assert!(document.undo());
        assert_eq!(document.variables[variable].value, 40.0);
        assert!((profile_width(&document, sketch) - 40.0).abs() < 1.0e-6);
    }

    #[test]
    fn native_roundtrip_preserves_variables_dimension_expr_and_replays_edit() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("variables.f3d");
        let mut document = Document::new();
        let variable = document.add_variable();
        assert!(document.update_variable(variable, "w".to_owned(), "40".to_owned()));
        let sketch = add_variable_rectangle(&mut document);
        document.save_to(&path).expect("save expression project");

        let mut loaded = Document::load_from(&path).expect("load expression project");
        assert_eq!(loaded.variables, document.variables);
        assert_eq!(loaded.sketches[0].constraints[8].expression(), Some("w"));
        assert!((profile_width(&loaded, sketch) - 40.0).abs() < 1.0e-6);
        assert!(loaded.update_variable(variable, "w".to_owned(), "60".to_owned()));
        assert!((profile_width(&loaded, sketch) - 60.0).abs() < 1.0e-6);
    }

    fn center(shape: &Shape) -> DVec3 {
        let bounds = aabb(shape);
        (bounds.min() + bounds.max()) * 0.5
    }

    #[test]
    fn scale_doubles_bounds_about_pivot_and_undoes() {
        let mut document = Document::new();
        let id = document.add_body("Box", Shape::box_from_corners(DVec3::ZERO, DVec3::ONE));
        let pivot = DVec3::splat(0.5);
        assert_eq!(document.apply_scale(&[id], 2.0, pivot), vec![id]);
        let bounds = aabb(&document.bodies[0].shape);
        assert!(bounds.min().distance(DVec3::splat(-0.5)) < 1.0e-6);
        assert!(bounds.max().distance(DVec3::splat(1.5)) < 1.0e-6);
        assert!(document.undo());
        let bounds = aabb(&document.bodies[0].shape);
        assert!(bounds.min().distance(DVec3::ZERO) < 1.0e-6);
        assert!(bounds.max().distance(DVec3::ONE) < 1.0e-6);
    }

    #[test]
    fn split_box_at_center_produces_two_half_y_extents() {
        let mut document = Document::new();
        let id = document.add_body(
            "Box",
            Shape::box_from_corners(DVec3::ZERO, DVec3::splat(2.0)),
        );
        let halves = document.apply_split(id, 1.0);
        assert_eq!(halves.len(), 2);
        let mut extents: Vec<_> = document
            .bodies
            .iter()
            .map(|body| {
                let bounds = aabb(&body.shape);
                (bounds.min().y, bounds.max().y)
            })
            .collect();
        extents.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert!((extents[0].0 - 0.0).abs() < 1.0e-6 && (extents[0].1 - 1.0).abs() < 1.0e-6);
        assert!((extents[1].0 - 1.0).abs() < 1.0e-6 && (extents[1].1 - 2.0).abs() < 1.0e-6);
    }

    #[test]
    fn align_matches_only_chosen_bbox_center_axes() {
        let mut document = Document::new();
        let first = document.add_body("First", Shape::box_from_corners(DVec3::ZERO, DVec3::ONE));
        let second = document.add_body(
            "Second",
            Shape::box_from_corners(DVec3::splat(10.0), DVec3::splat(12.0)),
        );
        assert_eq!(
            document.apply_align(&[first, second], [false, true, true]),
            vec![second]
        );
        let a = center(
            &document
                .bodies
                .iter()
                .find(|body| body.id == first)
                .unwrap()
                .shape,
        );
        let b = center(
            &document
                .bodies
                .iter()
                .find(|body| body.id == second)
                .unwrap()
                .shape,
        );
        assert!((a.y - b.y).abs() < 1.0e-6 && (a.z - b.z).abs() < 1.0e-6);
        assert!((b.x - 11.0).abs() < 1.0e-6);
    }

    fn add_rectangle(
        document: &mut Document,
        plane: SketchPlane,
        width: f64,
        height: f64,
    ) -> SketchId {
        let sketch = document.add_sketch(plane);
        let points = [
            glam::DVec2::ZERO,
            glam::DVec2::new(width, 0.0),
            glam::DVec2::new(width, height),
            glam::DVec2::new(0.0, height),
        ];
        assert!(document.add_sketch_entities(
            sketch,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            })
        ));
        sketch
    }

    #[test]
    fn undo_add_line_restores_entity_count() {
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            id,
            [SketchEntity::Line {
                a: glam::DVec2::ZERO,
                b: glam::DVec2::X,
            }]
        ));
        assert_eq!(document.sketches[0].entities.len(), 1);
        assert!(document.undo());
        assert!(document.sketches[0].entities.is_empty());
    }

    #[test]
    fn constraints_and_defined_state_survive_undo_redo() {
        use crate::constraint::{Constraint, EntityRef};

        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        document.sketches[0].pinned = vec![0, 1];
        assert!(document.add_sketch_entities_with_constraints(
            id,
            [SketchEntity::Line {
                a: glam::DVec2::ZERO,
                b: glam::DVec2::new(3.0, 0.2),
            }],
            [
                Constraint::Horizontal(EntityRef(0)),
                Constraint::Length {
                    line: EntityRef(0),
                    value: 4.0,
                    expr: None,
                    error: None,
                    reference: false,
                },
            ],
        ));
        assert_eq!(document.sketches[0].constraints.len(), 2);
        assert_eq!(document.sketches[0].defined, vec![true]);
        assert!(document.undo());
        assert!(document.sketches[0].entities.is_empty());
        assert!(document.redo());
        assert_eq!(document.sketches[0].constraints.len(), 2);
        assert_eq!(document.sketches[0].defined, vec![true]);
    }

    #[test]
    fn add_constraint_rejects_collapsed_contradiction() {
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            id,
            [SketchEntity::Line {
                a: glam::DVec2::ZERO,
                b: glam::DVec2::new(4.0, 2.0),
            }]
        ));
        assert!(document.add_constraint(id, Constraint::Horizontal(EntityRef(0))));
        let before = document.sketches[0].constraints.clone();
        assert!(!document.add_constraint(id, Constraint::Vertical(EntityRef(0))));
        assert_eq!(document.sketches[0].constraints, before);
    }

    #[test]
    fn length_dimension_update_replaces_existing_constraint() {
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            id,
            [SketchEntity::Line {
                a: glam::DVec2::ZERO,
                b: glam::DVec2::new(4.0, 0.0),
            }]
        ));
        assert!(document.set_dimension(id, 0, 6.0));
        let count = document.sketches[0].constraints.len();
        assert!(document.set_dimension(id, 0, 9.0));
        assert_eq!(document.sketches[0].constraints.len(), count);
        assert!(
            document.sketches[0]
                .constraints
                .contains(&Constraint::Length {
                    line: EntityRef(0),
                    value: 9.0,
                    expr: None,
                    error: None,
                    reference: false,
                })
        );
    }

    #[test]
    fn sketch_mirror_is_undoable_and_flushes_as_sketch_state() {
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            id,
            [
                SketchEntity::Line {
                    a: DVec2::new(2.0, 1.0),
                    b: DVec2::new(3.0, 2.0)
                },
                SketchEntity::Line {
                    a: DVec2::new(0.0, -5.0),
                    b: DVec2::new(0.0, 5.0)
                },
            ]
        ));
        assert!(document.mirror_sketch_entities(id, &[0], 1));
        assert_eq!(document.sketches[0].entities.len(), 3);
        assert!(document.undo());
        assert_eq!(document.sketches[0].entities.len(), 2);
        document.redo();
        let history = document.replayable_history();
        assert!(
            history.iter().any(
                |step| matches!(step.op, HistoryOp::SketchState { sketch, .. } if sketch == id)
            )
        );
    }

    #[test]
    fn per_entity_defined_state_separates_fixed_and_dangling_lines() {
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        document.sketches[0].pinned = vec![0, 1];
        assert!(document.add_sketch_entities_with_constraints(
            id,
            [
                SketchEntity::Line {
                    a: DVec2::ZERO,
                    b: DVec2::new(3.0, 0.2)
                },
                SketchEntity::Line {
                    a: DVec2::new(10.0, 10.0),
                    b: DVec2::new(12.0, 13.0)
                },
            ],
            [
                Constraint::Horizontal(EntityRef(0)),
                Constraint::Length {
                    line: EntityRef(0),
                    value: 4.0,
                    expr: None,
                    error: None,
                    reference: false
                },
            ]
        ));
        assert_eq!(document.sketches[0].defined, vec![true, false]);
    }

    #[test]
    fn drag_solve_keeps_constrained_rectangle_closed() {
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        let points = [
            glam::DVec2::new(0.0, 0.0),
            glam::DVec2::new(40.0, 0.0),
            glam::DVec2::new(40.0, 30.0),
            glam::DVec2::new(0.0, 30.0),
        ];
        let entities = (0..4).map(|index| SketchEntity::Line {
            a: points[index],
            b: points[(index + 1) % 4],
        });
        let constraints = (0..4).map(|index| Constraint::Coincident {
            a: PointRef {
                entity: index,
                point: 1,
            },
            b: PointRef {
                entity: (index + 1) % 4,
                point: 0,
            },
        });
        assert!(document.add_sketch_entities_with_constraints(id, entities, constraints));
        let start = document.sketches[0].clone();
        assert!(document.preview_sketch_drag(id, 0, glam::DVec2::new(5.0, 7.0), &start));
        assert!(document.finish_sketch_drag(id, start, true));
        let sketch = &document.sketches[0];
        for index in 0..4 {
            let SketchEntity::Line { b, .. } = sketch.entities[index].geo else {
                unreachable!()
            };
            let SketchEntity::Line { a, .. } = sketch.entities[(index + 1) % 4].geo else {
                unreachable!()
            };
            assert!(a.distance(b) < 1.0e-8);
        }
    }

    #[test]
    fn step_document_roundtrip_preserves_bounds() {
        let path = std::env::temp_dir().join(format!(
            "free3d-document-roundtrip-{}.step",
            std::process::id()
        ));
        let mut source = Document::new();
        source.add_body(
            "Box",
            Shape::box_from_corners(DVec3::new(-2.0, 3.0, 4.0), DVec3::new(8.0, 13.0, 24.0)),
        );
        source.export(&path).expect("write STEP");

        let mut imported = Document::new();
        imported.import_step(&path).expect("read STEP");
        let expected = aabb(&source.bodies[0].shape);
        let actual = aabb(&imported.bodies[0].shape);
        assert!((actual.min() - expected.min()).length() < 1.0e-6);
        assert!((actual.max() - expected.max()).length() < 1.0e-6);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn undo_redo_add_body_roundtrip() {
        let mut document = Document::new();
        let id = document.add_body("Box 1", box_shape());
        assert_eq!(document.bodies.len(), 1);
        assert!(document.undo());
        assert!(document.bodies.is_empty());
        assert!(document.redo());
        assert_eq!(document.bodies.len(), 1);
        assert_eq!(document.bodies[0].id, id);
    }

    #[test]
    fn selection_is_sanitized_after_undo() {
        let mut document = Document::new();
        let id = document.add_body("Box 1", box_shape());
        document.selection.items.push(SelItem::Body(id));
        assert!(document.undo());
        assert!(document.selection.items.is_empty());
    }

    #[test]
    fn undo_history_is_capped_at_64() {
        let mut document = Document::new();
        for index in 0..70 {
            document.add_body(format!("Box {index}"), box_shape());
        }
        assert_eq!(document.undo.len(), MAX_SNAPSHOTS);
    }

    #[test]
    fn apply_transform_translate_and_undo_roundtrip() {
        let mut document = Document::new();
        let id = document.add_body("Box 1", box_shape());
        let before = {
            let bounds = aabb(&document.bodies[0].shape);
            (bounds.min() + bounds.max()) * 0.5
        };
        let delta = DVec3::new(3.0, -4.0, 5.0);
        assert_eq!(
            document.apply_transform(&[id], TransformOp::Translate(delta)),
            vec![id]
        );
        let moved = {
            let bounds = aabb(&document.bodies[0].shape);
            (bounds.min() + bounds.max()) * 0.5
        };
        assert!((moved - before - delta).length() < 1.0e-6);
        assert!(document.undo());
        let restored = {
            let bounds = aabb(&document.bodies[0].shape);
            (bounds.min() + bounds.max()) * 0.5
        };
        assert!((restored - before).length() < 1.0e-6);
    }

    #[test]
    fn boolean_union_removes_tools_and_undo_restores_them() {
        let mut document = Document::new();
        let target = document.add_body(
            "Target",
            Shape::box_from_corners(DVec3::ZERO, DVec3::splat(10.0)),
        );
        let tool = document.add_body(
            "Tool",
            Shape::box_from_corners(DVec3::splat(5.0), DVec3::splat(15.0)),
        );
        assert!(document.apply_boolean(BooleanOp::Union, &[target, tool]));
        assert_eq!(document.bodies.len(), 1);
        assert_eq!(document.bodies[0].id, target);
        let bounds = aabb(&document.bodies[0].shape);
        assert!((bounds.min() - DVec3::ZERO).length() < 1.0e-6);
        assert!((bounds.max() - DVec3::splat(15.0)).length() < 1.0e-6);
        assert!(document.undo());
        assert_eq!(document.bodies.len(), 2);
        assert!(document.bodies.iter().any(|body| body.id == tool));
    }

    #[test]
    fn boolean_subtract_shrinks_target() {
        let mut document = Document::new();
        let target = document.add_body(
            "Target",
            Shape::box_from_corners(DVec3::ZERO, DVec3::splat(10.0)),
        );
        let tool = document.add_body(
            "Tool",
            Shape::box_from_corners(DVec3::new(5.0, -1.0, -1.0), DVec3::splat(11.0)),
        );
        assert!(document.apply_boolean(BooleanOp::Subtract, &[target, tool]));
        let bounds = aabb(&document.bodies[0].shape);
        assert!((bounds.max().x - 5.0).abs() < 1.0e-6);
    }

    #[test]
    fn fillet_cube_edge_adds_topology() {
        let mut document = Document::new();
        let id = document.add_body("Box", Shape::cube(10.0));
        let before_edges = document.bodies[0].shape.edge_count().unwrap();
        assert!(document.apply_dressup(
            id,
            DressUp::Fillet {
                radius: 1.0,
                end_radius: None,
                edge_indices: vec![0],
            }
        ));
        assert!(document.bodies[0].shape.edge_count().unwrap() > before_edges);
    }

    #[test]
    fn shell_box_top_adds_inner_faces_without_growing_bbox() {
        let mut document = Document::new();
        let id = document.add_body("Box", Shape::cube(10.0));
        let before_faces = document.bodies[0].shape.face_count().unwrap();
        let before = aabb(&document.bodies[0].shape);
        let shape = &document.bodies[0].shape;
        let top = (0..shape.face_count().unwrap())
            .filter_map(|index| Some((index, shape.face_center_of_mass(index).ok()?)))
            .max_by(|(_, a), (_, b)| a.z.total_cmp(&b.z))
            .map(|(index, _)| index as u32)
            .expect("box top face");
        assert!(document.apply_shell(id, &[top], 1.0));
        let after = aabb(&document.bodies[0].shape);
        assert!((after.min() - before.min()).length() < 1.0e-6);
        assert!((after.max() - before.max()).length() < 1.0e-6);
        assert!(document.bodies[0].shape.face_count().unwrap() > before_faces);
    }

    #[test]
    fn revolve_profile_is_symmetric_about_axis_and_undo_removes_body() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        let points = [
            glam::DVec2::new(10.0, -5.0),
            glam::DVec2::new(20.0, -5.0),
            glam::DVec2::new(20.0, 5.0),
            glam::DVec2::new(10.0, 5.0),
        ];
        assert!(document.add_sketch_entities(
            sketch,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            })
        ));
        let before = document.bodies.len();
        assert!(
            document
                .apply_revolve(
                    SelItem::Profile(sketch, 0),
                    DVec3::ZERO,
                    DVec3::Y,
                    360.0,
                    ExtrudeMode::NewBody,
                )
                .is_some()
        );
        let bounds = aabb(&document.bodies[before].shape);
        assert!((bounds.min().x + bounds.max().x).abs() < 1.0e-5);
        assert!((bounds.min().z + bounds.max().z).abs() < 1.0e-5);
        assert!(document.undo());
        assert_eq!(document.bodies.len(), before);
    }

    #[test]
    fn revolve_about_picked_world_x_axis_has_expected_bounds() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        let points = [
            glam::DVec2::new(-5.0, 10.0),
            glam::DVec2::new(5.0, 10.0),
            glam::DVec2::new(5.0, 20.0),
            glam::DVec2::new(-5.0, 20.0),
        ];
        assert!(document.add_sketch_entities(
            sketch,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            })
        ));
        let revolved = document
            .apply_revolve(
                SelItem::Profile(sketch, 0),
                DVec3::ZERO,
                DVec3::X,
                360.0,
                ExtrudeMode::NewBody,
            )
            .expect("world-X revolve");
        let body = document
            .bodies
            .iter()
            .find(|body| body.id == revolved)
            .expect("revolved body");
        let bounds = aabb(&body.shape);
        assert!(
            (bounds.min() - DVec3::new(-5.0, -20.0, -20.0)).length() < 0.25,
            "minimum was {:?}",
            bounds.min()
        );
        assert!(
            (bounds.max() - DVec3::new(5.0, 20.0, 20.0)).length() < 0.25,
            "maximum was {:?}",
            bounds.max()
        );
    }

    #[test]
    fn sweep_circle_along_l_shaped_open_chain_spans_path() {
        let mut document = Document::new();
        let section_plane = SketchPlane {
            origin: DVec3::ZERO,
            x_axis: DVec3::Y,
            y_axis: DVec3::Z,
        };
        let section = document.add_sketch(section_plane);
        assert!(document.add_sketch_entities(
            section,
            [SketchEntity::Circle {
                center: glam::DVec2::ZERO,
                radius: 2.0,
            }]
        ));
        let path = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            path,
            [
                SketchEntity::Line {
                    a: glam::DVec2::ZERO,
                    b: glam::DVec2::new(20.0, 0.0),
                },
                SketchEntity::Line {
                    a: glam::DVec2::new(20.0, 0.0),
                    b: glam::DVec2::new(20.0, 30.0),
                },
            ]
        ));
        let body = document
            .apply_sweep(
                (section, 0),
                PathRef::OpenChain {
                    sketch: path,
                    entity_indices: vec![0, 1],
                },
            )
            .expect("valid open sweep");
        let bounds = aabb(
            &document
                .bodies
                .iter()
                .find(|candidate| candidate.id == body)
                .expect("sweep body")
                .shape,
        );
        assert!(bounds.max().x > 19.0);
        assert!(bounds.max().y > 29.0);
        assert!(bounds.min().z < -1.9 && bounds.max().z > 1.9);
    }

    #[test]
    fn sweep_profile_along_closed_profile_path_creates_body() {
        let mut document = Document::new();
        let section = document.add_sketch(SketchPlane {
            origin: DVec3::new(10.0, 0.0, 0.0),
            x_axis: DVec3::X,
            y_axis: DVec3::Z,
        });
        assert!(document.add_sketch_entities(
            section,
            [SketchEntity::Circle {
                center: glam::DVec2::ZERO,
                radius: 1.0,
            }]
        ));
        let path = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            path,
            [SketchEntity::Circle {
                center: glam::DVec2::ZERO,
                radius: 10.0,
            }]
        ));
        let body = document.apply_sweep(
            (section, 0),
            PathRef::Profile {
                sketch: path,
                profile_index: 0,
            },
        );
        assert!(body.is_some(), "closed profile path should sweep");
        assert_eq!(document.bodies.len(), 1);
    }

    #[test]
    fn loft_between_offset_rectangles_spans_fifty_units() {
        let mut document = Document::new();
        let lower = add_rectangle(&mut document, SketchPlane::xy(), 20.0, 10.0);
        let upper = add_rectangle(
            &mut document,
            SketchPlane {
                origin: DVec3::new(2.0, 1.0, 50.0),
                ..SketchPlane::xy()
            },
            12.0,
            8.0,
        );
        let body = document
            .apply_loft(&[(lower, 0), (upper, 0)])
            .expect("valid solid loft");
        let bounds = aabb(
            &document
                .bodies
                .iter()
                .find(|candidate| candidate.id == body)
                .expect("loft body")
                .shape,
        );
        // OCCT's default AABB adds a 0.5 modeling tolerance around the shape.
        assert!(
            (-0.6..=0.0).contains(&bounds.min().z),
            "loft min: {}",
            bounds.min()
        );
        assert!(
            (50.0..50.6).contains(&bounds.max().z),
            "loft max: {}",
            bounds.max()
        );
    }

    #[test]
    fn invalid_sweep_and_loft_leave_document_unchanged() {
        let mut document = Document::new();
        let sketch = add_rectangle(&mut document, SketchPlane::xy(), 10.0, 10.0);
        let before = (
            document.bodies.len(),
            document.history.clone(),
            document.next_id,
            document.undo.len(),
            document.scene_epoch,
        );
        assert!(
            document
                .apply_sweep(
                    (sketch, 99),
                    PathRef::Profile {
                        sketch,
                        profile_index: 0,
                    },
                )
                .is_none()
        );
        assert!(document.apply_loft(&[(sketch, 0)]).is_none());
        assert_eq!(document.bodies.len(), before.0);
        assert_eq!(document.history, before.1);
        assert_eq!(document.next_id, before.2);
        assert_eq!(document.undo.len(), before.3);
        assert_eq!(document.scene_epoch, before.4);
    }

    #[test]
    fn degenerate_loft_rejection_leaves_document_unchanged() {
        let mut document = Document::new();
        let sketch = add_rectangle(&mut document, SketchPlane::xy(), 10.0, 10.0);
        let before = (
            document.bodies.len(),
            document.history.clone(),
            document.next_id,
            document.undo.len(),
            document.scene_epoch,
        );
        assert!(document.apply_loft(&[(sketch, 0), (sketch, 0)]).is_none());
        assert_eq!(document.bodies.len(), before.0);
        assert_eq!(document.history, before.1);
        assert_eq!(document.next_id, before.2);
        assert_eq!(document.undo.len(), before.3);
        assert_eq!(document.scene_epoch, before.4);
    }

    #[test]
    fn mirror_copies_body_across_zx_and_undo_removes_copy() {
        let mut document = Document::new();
        let id = document.add_body(
            "Offset",
            Shape::box_from_corners(DVec3::new(1.0, 2.0, 3.0), DVec3::new(4.0, 5.0, 6.0)),
        );
        let copies = document.apply_mirror(&[id], DVec3::ZERO, DVec3::Y);
        assert_eq!(copies.len(), 1);
        assert_eq!(document.bodies.len(), 2);
        let bounds = aabb(&document.bodies[1].shape);
        assert!((bounds.min() - DVec3::new(1.0, -5.0, 3.0)).length() < 1.0e-6);
        assert!((bounds.max() - DVec3::new(4.0, -2.0, 6.0)).length() < 1.0e-6);
        assert!(document.undo());
        assert_eq!(document.bodies.len(), 1);
    }

    #[test]
    fn mirror_across_picked_face_plane_has_expected_bounds() {
        let mut document = Document::new();
        let reference = document.add_body("Reference", Shape::cube(10.0));
        let shape = &document
            .bodies
            .iter()
            .find(|body| body.id == reference)
            .expect("reference body")
            .shape;
        let face_index = (0..shape.face_count().unwrap())
            .filter_map(|index| Some((index, shape.face_center_of_mass(index).ok()?)))
            .max_by(|(_, left), (_, right)| left.x.total_cmp(&right.x))
            .map(|(index, _)| index as u32)
            .expect("right face");
        let plane = SketchPlane::from_face(
            &document
                .bodies
                .iter()
                .find(|body| body.id == reference)
                .expect("reference body")
                .shape,
            face_index,
        )
        .expect("planar face");
        let source = document.add_body(
            "Source",
            Shape::box_from_corners(DVec3::new(1.0, 2.0, 3.0), DVec3::new(4.0, 5.0, 6.0)),
        );
        let copies = document.apply_mirror(&[source], plane.origin, plane.normal());
        assert_eq!(copies.len(), 1);
        let mirrored = document
            .bodies
            .iter()
            .find(|body| body.id == copies[0])
            .expect("mirror copy");
        let bounds = aabb(&mirrored.shape);
        assert!((bounds.min() - DVec3::new(16.0, 2.0, 3.0)).length() < 1.0e-5);
        assert!((bounds.max() - DVec3::new(19.0, 5.0, 6.0)).length() < 1.0e-5);
    }

    #[test]
    fn linear_pattern_makes_two_spaced_copies_and_undo_removes_them() {
        let mut document = Document::new();
        let id = document.add_body("Seed", box_shape());
        let copies = document.apply_linear_pattern(&[id], DVec3::ZERO, DVec3::X, 3, 25.0);
        assert_eq!(copies.len(), 2);
        assert_eq!(document.bodies.len(), 3);
        for (index, expected_x) in [25.0, 50.0].into_iter().enumerate() {
            let bounds = aabb(&document.bodies[index + 1].shape);
            assert!((bounds.min().x - expected_x).abs() < 1.0e-6);
            assert!((bounds.max().x - (expected_x + 1.0)).abs() < 1.0e-6);
        }
        assert!(document.undo());
        assert_eq!(document.bodies.len(), 1);
    }

    #[test]
    fn circular_pattern_makes_three_rotated_copies_and_undo_removes_them() {
        let mut document = Document::new();
        let id = document.add_body(
            "Seed",
            Shape::box_from_corners(DVec3::new(10.0, 0.0, 0.0), DVec3::new(12.0, 2.0, 2.0)),
        );
        let copies = document.apply_circular_pattern(&[id], DVec3::ZERO, DVec3::Z, 4);
        assert_eq!(copies.len(), 3);
        assert_eq!(document.bodies.len(), 4);
        let quarter_turn = aabb(&document.bodies[1].shape);
        assert!((quarter_turn.min() - DVec3::new(-2.0, 10.0, 0.0)).length() < 1.0e-6);
        assert!((quarter_turn.max() - DVec3::new(0.0, 12.0, 2.0)).length() < 1.0e-6);
        assert!(document.undo());
        assert_eq!(document.bodies.len(), 1);
    }

    #[test]
    fn importing_two_solid_step_creates_two_named_bodies() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("assembly.step");
        let first = Shape::box_from_corners(DVec3::ZERO, DVec3::ONE).unwrap();
        let second = Shape::box_from_corners(DVec3::splat(3.0), DVec3::splat(4.0)).unwrap();
        Shape::write_step_refs([&first, &second], &path).unwrap();
        let mut document = Document::new();
        document.import_step(&path).unwrap();
        assert_eq!(document.bodies.len(), 2);
        assert_eq!(document.bodies[0].name, "assembly 1");
        assert_eq!(document.bodies[1].name, "assembly 2");
    }

    #[test]
    fn native_roundtrip_preserves_model_and_replay_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("roundtrip.f3d");
        let mut source = Document::new();
        source.add_primitive(PrimitiveKind::Box {
            min: DVec3::new(-2.0, 3.0, 4.0),
            max: DVec3::new(8.0, 13.0, 24.0),
        });
        let sketch = source.add_sketch(SketchPlane::xy());
        assert!(source.add_sketch_entities_with_constraints(
            sketch,
            [SketchEntity::Line {
                a: DVec2::ZERO,
                b: DVec2::new(10.0, 0.2),
            }],
            [Constraint::Horizontal(EntityRef(0))],
        ));
        source.sketches[0].pinned = vec![0, 1];
        source.add_construction_plane(SketchPlane {
            origin: DVec3::new(0.0, 0.0, 17.0),
            ..SketchPlane::xy()
        });
        let view = source.drawing.add_view(
            crate::drawing::Projection::Front,
            DVec2::new(120.0, 80.0),
            0.5,
        );
        source
            .drawing
            .sheet_mut()
            .dims
            .push(crate::drawing::DrawingDim::linear(
                DVec2::new(10.0, 20.0),
                DVec2::new(30.0, 20.0),
                8.0,
                0.5,
            ));
        source.drawing.sheet_mut().title.project_name = crate::i18n::t("Assembly Project").into();
        source.drawing.sheet_mut().title.author = "Free3D".into();
        source.drawing.add_sheet();
        source.drawing.sheet_mut().title.drawing_number = "F3D-002".into();
        source.drawing.active_sheet = 0;
        source.save_to(&path).expect("save native project");

        let loaded = Document::load_from(&path).expect("load native project");
        assert_eq!(loaded.bodies.len(), source.bodies.len());
        assert_eq!(loaded.sketches[0].entities.len(), 1);
        assert_eq!(loaded.sketches[0].constraints.len(), 1);
        assert_eq!(loaded.sketches[0].pinned, vec![0, 1]);
        assert_eq!(loaded.construction_planes.len(), 1);
        assert_eq!(loaded.drawing, source.drawing);
        assert_eq!(loaded.drawing.sheet().views[0].id, view);
        assert_eq!(loaded.drawing.sheets.len(), 2);
        assert_eq!(
            loaded.drawing.sheets[0].title.project_name,
            crate::i18n::t("Assembly Project")
        );
        assert_eq!(loaded.drawing.sheets[1].title.drawing_number, "F3D-002");
        assert_eq!(loaded.history.len(), source.history.len());
        assert_eq!(loaded.active_sketch, source.active_sketch);
        assert!(loaded.undo.is_empty() && loaded.redo.is_empty());
        let expected = aabb(&source.bodies[0].shape);
        let actual = aabb(&loaded.bodies[0].shape);
        assert!((actual.min() - expected.min()).length() < 1.0e-6);
        assert!((actual.max() - expected.max()).length() < 1.0e-6);

        let replayed = crate::history::replay(&loaded.history).expect("replay loaded history");
        let replayed = aabb(&replayed.bodies[0].shape);
        assert!((replayed.min() - expected.min()).length() < 1.0e-6);
        assert!((replayed.max() - expected.max()).length() < 1.0e-6);
    }

    #[test]
    fn native_roundtrip_preserves_element_fingerprints() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fingerprints.f3d");
        let mut source = Document::new();
        let body = source.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        assert!(source.apply_dressup(
            body,
            DressUp::Fillet {
                radius: 1.0,
                end_radius: None,
                edge_indices: vec![0],
            },
        ));
        source.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(loaded.history, source.history);
        let HistoryOp::Fillet { edges, .. } = &loaded.history[1].op else {
            panic!("fillet history step");
        };
        assert!(edges[0].print.is_some());
    }

    #[test]
    fn native_load_rejects_newer_version_and_corrupt_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("version.f3d");
        let mut document = Document::new();
        document.save_to(&path).unwrap();
        std::fs::write(&path, br#"{"format":"free3d","version":99}"#).unwrap();
        let error = Document::load_from(&path)
            .err()
            .expect("newer version must fail");
        assert!(error.starts_with(crate::i18n::t("Unsupported file version")));

        std::fs::write(&path, b"{ definitely not json").unwrap();
        assert!(Document::load_from(&path).is_err());
    }

    #[test]
    fn revision_tracks_commits_and_undo_to_saved_state() {
        let mut document = Document::new();
        let saved_revision = document.revision;
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        assert_ne!(document.revision, saved_revision);
        assert!(document.undo());
        assert_eq!(document.revision, saved_revision);
        assert!(document.redo());
        assert_ne!(document.revision, saved_revision);
    }

    fn top_face(document: &Document, body: BodyId) -> (u32, DVec3) {
        let shape = &document
            .bodies
            .iter()
            .find(|item| item.id == body)
            .unwrap()
            .shape;
        (0..shape.face_count().unwrap())
            .filter_map(|index| Some((index as u32, shape.face_center_of_mass(index).ok()?)))
            .max_by(|(_, left), (_, right)| left.z.total_cmp(&right.z))
            .expect("box top face")
    }

    #[test]
    fn hole_variants_cut_box_without_changing_bounds() {
        let cases = [
            (HoleKind::Through, HoleCut::None, 1usize),
            (HoleKind::Blind { depth: 4.0.into() }, HoleCut::None, 2),
            (
                HoleKind::Through,
                HoleCut::Counterbore {
                    diameter: 5.0.into(),
                    depth: 2.0.into(),
                },
                3,
            ),
            (
                HoleKind::Blind { depth: 6.0.into() },
                HoleCut::Countersink {
                    diameter: 5.0.into(),
                    angle_degrees: 90.0.into(),
                },
                3,
            ),
        ];
        for (kind, cut, minimum_delta) in cases {
            let mut document = Document::new();
            let body = document.add_primitive(PrimitiveKind::Box {
                min: DVec3::ZERO,
                max: DVec3::splat(10.0),
            });
            let before = aabb(&document.bodies[0].shape);
            let before_faces = document.bodies[0].shape.face_count().unwrap();
            let (face, at) = top_face(&document, body);
            assert!(document.apply_hole(body, face, at, 3.0, kind.clone(), cut));
            let after = aabb(&document.bodies[0].shape);
            assert!(
                (after.min() - before.min()).abs().max_element() < 1.0e-1,
                "{kind:?}: before {:?}, after {:?}",
                before.min(),
                after.min()
            );
            assert!(
                (after.max() - before.max()).abs().max_element() < 1.0e-1,
                "{kind:?}: before {:?}, after {:?}",
                before.max(),
                after.max()
            );
            assert!(document.bodies[0].shape.face_count().unwrap() >= before_faces + minimum_delta);
            if matches!(kind, HoleKind::Through) {
                let hits = document.bodies[0]
                    .shape
                    .ray_hits(at + DVec3::Z * 2.0, DVec3::NEG_Z)
                    .expect("hole-axis ray");
                assert!(hits.is_empty(), "through bore axis must remain open");
            }
            let replayed = crate::history::replay(&document.history).expect("hole replay");
            assert_eq!(
                replayed.bodies[0].shape.face_count().unwrap(),
                document.bodies[0].shape.face_count().unwrap()
            );
        }
    }

    #[test]
    fn subtract_revolve_groove_changes_supported_box() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let sketch = document.add_sketch_with_support(
            SketchPlane {
                origin: DVec3::ZERO,
                x_axis: DVec3::X,
                y_axis: DVec3::Z,
            },
            Some(body),
        );
        let points = [
            DVec2::new(3.0, 2.0),
            DVec2::new(4.0, 2.0),
            DVec2::new(4.0, 8.0),
            DVec2::new(3.0, 8.0),
        ];
        assert!(document.add_sketch_entities(
            sketch,
            (0..4).map(|index| SketchEntity::Line {
                a: points[index],
                b: points[(index + 1) % 4],
            }),
        ));
        let before_faces = document.bodies[0].shape.face_count().unwrap();
        assert_eq!(
            document.apply_revolve(
                SelItem::Profile(sketch, 0),
                DVec3::ZERO,
                DVec3::Z,
                360.0,
                ExtrudeMode::Subtract,
            ),
            Some(body)
        );
        assert!(document.bodies[0].shape.face_count().unwrap() > before_faces);
        let replayed = crate::history::replay(&document.history).expect("groove replay");
        assert_eq!(
            replayed.bodies[0].shape.face_count().unwrap(),
            document.bodies[0].shape.face_count().unwrap()
        );
    }

    #[test]
    fn new_primitive_bounds_and_replay_are_deterministic() {
        let kinds = [
            PrimitiveKind::Ellipsoid {
                center: DVec3::ZERO,
                radii: DVec3::new(4.0, 3.0, 2.0),
            },
            PrimitiveKind::Prism {
                center: DVec3::ZERO,
                radius: 5.0,
                sides: 6,
                height: 7.0,
            },
            PrimitiveKind::Wedge {
                origin: DVec3::ZERO,
                dx: 8.0,
                dy: 6.0,
                dz: 4.0,
                top_dx: 3.0,
            },
        ];
        for kind in kinds {
            let mut document = Document::new();
            document.add_primitive(kind);
            let expected = aabb(&document.bodies[0].shape);
            assert!((expected.max() - expected.min()).min_element() > 0.0);
            let replayed = crate::history::replay(&document.history).expect("primitive replay");
            let actual = aabb(&replayed.bodies[0].shape);
            assert!((actual.min() - expected.min()).length() < 1.0e-6);
            assert!((actual.max() - expected.max()).length() < 1.0e-6);
            assert_eq!(replayed.bodies[0].id, document.bodies[0].id);
        }
    }

    #[test]
    fn draft_box_sides_changes_horizontal_extent() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let shape = document.bodies[0].shape.clone();
        let sides: Vec<u32> = (0..shape.face_count().unwrap())
            .filter_map(|index| {
                let center = shape.face_center_of_mass(index).ok()?;
                ((center.z - 5.0).abs() < 1.0e-5).then_some(index as u32)
            })
            .collect();
        let before = aabb(&shape);
        assert_eq!(sides.len(), 4);
        assert!(document.apply_draft(body, &sides, DVec3::Z, DVec3::ZERO, DVec3::Z, -5.0,));
        let after = aabb(&document.bodies[0].shape);
        assert!(((after.max().x - after.min().x) - (before.max().x - before.min().x)).abs() > 0.1);
        crate::history::replay(&document.history).expect("draft replay");
    }

    #[test]
    fn replace_face_extends_and_trims_box_and_replays() {
        for (target_z, expected_max) in [(15.0, 15.0), (7.0, 7.0)] {
            let mut document = Document::new();
            let body = document.add_primitive(PrimitiveKind::Box {
                min: DVec3::ZERO,
                max: DVec3::splat(10.0),
            });
            let (face, _) = top_face(&document, body);
            assert!(document.apply_replace_face(
                body,
                face,
                DVec3::new(0.0, 0.0, target_z),
                DVec3::Z,
            ));
            let bounds = aabb(&document.bodies[0].shape);
            assert!((bounds.min().z - 0.0).abs() < 1.0e-5);
            assert!((bounds.max().z - expected_max).abs() < 1.0e-5);
            let replayed = crate::history::replay(&document.history).expect("replace-face replay");
            let replayed = aabb(&replayed.bodies[0].shape);
            assert!((replayed.max().z - expected_max).abs() < 1.0e-5);
        }
    }

    #[test]
    fn project_box_top_edge_to_xy_creates_matching_construction_line() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let shape = document.bodies[0].shape.clone();
        let edge = (0..shape.edge_count().unwrap())
            .find(|&index| {
                let a = shape.edge_start_point(index).unwrap();
                let b = shape.edge_end_point(index).unwrap();
                (a.z - 10.0).abs() < 1.0e-6 && (b.z - 10.0).abs() < 1.0e-6
            })
            .expect("top edge");
        let expected_a = SketchPlane::xy().to_local(shape.edge_start_point(edge).unwrap());
        let expected_b = SketchPlane::xy().to_local(shape.edge_end_point(edge).unwrap());
        let sketch = document.add_sketch(SketchPlane::xy());
        assert!(document.project_to_sketch(sketch, SelItem::Edge(body, edge as u32)));
        let item = &document.sketches.last().unwrap().entities[0];
        assert!(item.construction);
        let SketchEntity::Line { a, b } = item.geo else {
            panic!("straight box edge must project as a line");
        };
        assert!(
            (a.distance(expected_a) < 1.0e-6 && b.distance(expected_b) < 1.0e-6)
                || (a.distance(expected_b) < 1.0e-6 && b.distance(expected_a) < 1.0e-6)
        );
        let history = document.replayable_history();
        let replayed = crate::history::replay(&history).expect("projection replay");
        assert!(replayed.sketches[0].entities[0].construction);
    }

    #[test]
    fn multi_transform_keeps_original_and_places_copies_at_multiples() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        let copies =
            document.apply_multi_transform(&[body], TransformOp::Translate(DVec3::X * 20.0), 3);
        assert_eq!(copies.len(), 2);
        assert_eq!(document.bodies.len(), 3);
        let mins: Vec<_> = document
            .bodies
            .iter()
            .map(|body| aabb(&body.shape).min().x)
            .collect();
        assert!(
            mins.iter()
                .zip([0.0, 20.0, 40.0])
                .all(|(actual, expected)| (*actual - expected).abs() < 1.0e-5)
        );
        assert_eq!(document.selection.items.len(), 3);
        let replayed = crate::history::replay(&document.history).expect("multi-transform replay");
        assert_eq!(replayed.bodies.len(), 3);
        assert!((aabb(&replayed.bodies[2].shape).min().x - 40.0).abs() < 1.0e-6);
    }

    #[test]
    fn helix_spring_has_expected_height_and_horizontal_extent() {
        let mut document = Document::new();
        let radius = 10.0;
        let pitch = 6.0;
        let turns = 3.0;
        let wire = 1.0;
        document
            .apply_helix(DVec3::ZERO, DVec3::Z, radius, pitch, turns, wire, false)
            .expect("helix spring");
        let bounds = aabb(&document.bodies[0].shape);
        let size = bounds.max() - bounds.min();
        assert!(
            (size.z - pitch * turns).abs() <= wire * 3.0,
            "spring size {size:?}, expected height {}",
            pitch * turns
        );
        assert!((size.x - 2.0 * (radius + wire)).abs() < wire);
        assert!((size.y - 2.0 * (radius + wire)).abs() < wire);
        let replayed = crate::history::replay(&document.history).expect("helix replay");
        assert_eq!(replayed.bodies.len(), 1);
    }

    #[test]
    fn construction_axis_and_point_persist_and_replay() {
        let mut document = Document::new();
        document
            .add_construction_axis(DVec3::new(1.0, 2.0, 3.0), DVec3::Y)
            .unwrap();
        document
            .add_construction_point(DVec3::new(4.0, 5.0, 6.0))
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("datums.f3d");
        document.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(loaded.construction_axes, document.construction_axes);
        assert_eq!(loaded.construction_points, document.construction_points);
        let replayed = crate::history::replay(&loaded.history).expect("datum replay");
        assert_eq!(replayed.construction_axes, loaded.construction_axes);
        assert_eq!(replayed.construction_points, loaded.construction_points);
    }

    #[test]
    fn mesh_exports_have_expected_topology_and_buffers() {
        let directory = tempfile::tempdir().unwrap();
        let mut document = Document::new();
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        let obj = directory.path().join("box.obj");
        document.export(&obj).unwrap();
        let text = std::fs::read_to_string(obj).unwrap();
        assert!(text.lines().filter(|line| line.starts_with("v ")).count() >= 8);
        assert!(text.lines().filter(|line| line.starts_with("f ")).count() >= 12);
        let three_mf = directory.path().join("box.3mf");
        document.export(&three_mf).unwrap();
        let archive = std::fs::read(three_mf).unwrap();
        let archive = String::from_utf8_lossy(&archive);
        assert!(archive.contains("3dmodel.model"));
        assert_eq!(
            archive.matches("<vertex ").count(),
            document.bodies[0].shape.mesh(0.1).unwrap().positions.len()
        );
        let gltf = directory.path().join("box.gltf");
        document.export(&gltf).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(gltf).unwrap()).unwrap();
        assert_eq!(json["asset"]["version"], "2.0");
        let buffer = &json["buffers"][0];
        let uri = buffer["uri"].as_str().unwrap();
        let encoded = uri.split_once(',').unwrap().1;
        assert_eq!(
            BASE64.decode(encoded).unwrap().len(),
            buffer["byteLength"].as_u64().unwrap() as usize
        );
    }

    #[test]
    fn iges_box_roundtrip_preserves_bounds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("box.iges");
        let mut source = Document::new();
        source.add_primitive(PrimitiveKind::Box {
            min: DVec3::new(1.0, 2.0, 3.0),
            max: DVec3::new(11.0, 12.0, 13.0),
        });
        source.export(&path).unwrap();
        let mut loaded = Document::new();
        loaded.import_file(&path).unwrap();
        assert_eq!(loaded.bodies.len(), 1);
        let a = aabb(&source.bodies[0].shape);
        let b = aabb(&loaded.bodies[0].shape);
        assert!((a.min() - b.min()).length() < 1e-5);
        assert!((a.max() - b.max()).length() < 1e-5);
    }

    #[test]
    fn dxf_line_circle_arc_roundtrip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sketch.dxf");
        let mut document = Document::new();
        let id = document.add_sketch(SketchPlane::xy());
        document.add_sketch_entities(
            id,
            [
                SketchEntity::Line {
                    a: DVec2::new(1.0, 2.0),
                    b: DVec2::new(3.0, 4.0),
                },
                SketchEntity::Circle {
                    center: DVec2::new(5.0, 6.0),
                    radius: 7.0,
                },
                SketchEntity::Arc {
                    start: DVec2::new(10.0, 0.0),
                    mid: DVec2::new(0.0, 10.0),
                    end: DVec2::new(-10.0, 0.0),
                },
            ],
        );
        document.export(&path).unwrap();
        let mut loaded = Document::new();
        loaded.import_file(&path).unwrap();
        assert_eq!(loaded.sketches[0].entities.len(), 3);
        assert_eq!(
            loaded.sketches[0].entities[0].geo,
            document.sketches[0].entities[0].geo
        );
        assert_eq!(
            loaded.sketches[0].entities[1].geo,
            document.sketches[0].entities[1].geo
        );
        let SketchEntity::Arc { start, mid, end } = &loaded.sketches[0].entities[2].geo else {
            panic!()
        };
        assert!(start.distance(DVec2::new(10.0, 0.0)) < 1e-6);
        assert!(mid.distance(DVec2::new(0.0, 10.0)) < 1e-6);
        assert!(end.distance(DVec2::new(-10.0, 0.0)) < 1e-6);
    }

    #[test]
    fn reference_image_bytes_survive_project_roundtrip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.f3d");
        let bytes = vec![0, 1, 2, 3, 254, 255];
        let mut document = Document::new();
        document.add_reference_image("plan", bytes.clone(), 125.0);
        document.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(loaded.reference_images[0].bytes, bytes);
        assert_eq!(loaded.reference_images[0].width_mm, 125.0);
        let replayed = crate::history::replay(&loaded.history).expect("reference image replay");
        assert_eq!(replayed.reference_images[0].bytes, bytes);
    }

    #[test]
    fn hsl_to_rgb_converts_primary_colors() {
        assert_eq!(hsl_to_rgb(0.0, 100.0, 50.0), [1.0, 0.0, 0.0]);
        assert_eq!(hsl_to_rgb(120.0, 100.0, 50.0), [0.0, 1.0, 0.0]);
        assert_eq!(hsl_to_rgb(240.0, 100.0, 50.0), [0.0, 0.0, 1.0]);
        let gray = hsl_to_rgb(42.0, 0.0, 25.0);
        assert!(
            gray.into_iter()
                .all(|channel| (channel - 0.25).abs() < 1.0e-6)
        );
    }

    #[test]
    fn old_project_without_material_uses_legacy_default() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("old.f3d");
        let mut document = Document::new();
        document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        document.save_to(&path).unwrap();
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["bodies"][0]
            .as_object_mut()
            .unwrap()
            .remove("material");
        json["bodies"][0].as_object_mut().unwrap().remove("kind");
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(loaded.bodies[0].material, Material::default());
        assert_eq!(loaded.bodies[0].kind, BodyKind::Solid);
    }

    #[test]
    fn set_material_is_undoable_and_replayable() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        let material = Material {
            base_color: [0.8, 0.1, 0.2],
            metallic: 0.7,
            roughness: 0.24,
        };
        document.set_material(body, material);
        assert_eq!(document.bodies[0].material, material);
        assert!(matches!(
            document.history.last().map(|step| &step.op),
            Some(HistoryOp::SetMaterial { body: id, material: value })
                if *id == body && *value == material
        ));
        assert!(document.undo());
        assert_eq!(document.bodies[0].material, Material::default());
        assert!(document.redo());
        assert_eq!(document.bodies[0].material, material);
        let replayed = crate::history::replay(&document.history).unwrap();
        assert_eq!(replayed.bodies[0].material, material);
    }

    #[test]
    fn native_roundtrip_preserves_body_materials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("materials.f3d");
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Cylinder {
            origin: DVec3::ZERO,
            radius: 2.0,
            axis: DVec3::Z,
            height: 5.0,
        });
        let material = Material {
            base_color: [0.18, 0.42, 0.76],
            metallic: 0.85,
            roughness: 0.34,
        };
        document.set_material(body, material);
        document.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(loaded.bodies[0].material, material);
    }

    fn face_edges(shape: &Shape, face: u32) -> Vec<u32> {
        (0..shape.edge_count().unwrap())
            .filter(|edge| {
                shape
                    .face_contains_edge(face as usize, *edge)
                    .unwrap_or(false)
            })
            .map(|edge| edge as u32)
            .collect()
    }

    #[test]
    fn open_chain_extrude_creates_surface_with_expected_bounds() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        document.add_sketch_entities(
            sketch,
            [
                SketchEntity::Line {
                    a: DVec2::ZERO,
                    b: DVec2::new(10.0, 0.0),
                },
                SketchEntity::Line {
                    a: DVec2::new(10.0, 0.0),
                    b: DVec2::new(10.0, 5.0),
                },
            ],
        );
        let body = document
            .apply_open_chain_extrude(
                sketch,
                &[0, 1],
                20.0,
                0.0,
                crate::tools::extrude::ExtrudeSideMode::OneSided,
            )
            .expect("open chain extrusion");
        let body = document.bodies.iter().find(|item| item.id == body).unwrap();
        assert_eq!(body.kind, BodyKind::Surface);
        let (minimum, maximum) = body.shape.aabb().unwrap();
        assert!(minimum.distance(DVec3::ZERO) < 1.0e-6);
        assert!(maximum.distance(DVec3::new(10.0, 5.0, 20.0)) < 1.0e-6);
        let replayed = crate::history::replay(&document.replayable_history()).unwrap();
        assert_eq!(replayed.bodies[0].kind, BodyKind::Surface);
    }

    #[test]
    fn open_chain_revolve_creates_replayable_surface() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        document.add_sketch_entities(
            sketch,
            [SketchEntity::Line {
                a: DVec2::new(5.0, 0.0),
                b: DVec2::new(5.0, 10.0),
            }],
        );
        let body = document
            .apply_open_chain_revolve(sketch, &[0], DVec3::ZERO, DVec3::Y, 180.0)
            .expect("open chain revolution");
        assert_eq!(
            document
                .bodies
                .iter()
                .find(|item| item.id == body)
                .unwrap()
                .kind,
            BodyKind::Surface
        );
        let replayed = crate::history::replay(&document.replayable_history()).unwrap();
        assert_eq!(replayed.bodies[0].kind, BodyKind::Surface);
    }

    #[test]
    fn patch_of_box_face_boundary_matches_face_bounds() {
        let mut document = Document::new();
        let box_id = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let (face, _) = top_face(&document, box_id);
        let edges = face_edges(&document.bodies[0].shape, face);
        let patch = document.apply_patch(box_id, &edges).expect("planar patch");
        let patch = document
            .bodies
            .iter()
            .find(|body| body.id == patch)
            .unwrap();
        assert_eq!(patch.kind, BodyKind::Surface);
        let (minimum, maximum) = patch.shape.aabb().unwrap();
        assert!(minimum.distance(DVec3::new(0.0, 0.0, 10.0)) < 1.0e-6);
        assert!(maximum.distance(DVec3::new(10.0, 10.0, 10.0)) < 1.0e-6);
    }

    #[test]
    fn six_box_face_patches_stitch_to_closed_solid() {
        let mut document = Document::new();
        let box_id = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let source = Arc::clone(&document.bodies[0].shape);
        let patches: Vec<_> = (0..source.face_count().unwrap())
            .map(|face| {
                document
                    .apply_patch(box_id, &face_edges(&source, face as u32))
                    .unwrap_or_else(|| panic!("box face {face} patch"))
            })
            .collect();
        let stitched = document.apply_stitch(&patches).expect("closed sewing");
        let stitched = document
            .bodies
            .iter()
            .find(|body| body.id == stitched)
            .unwrap();
        assert_eq!(stitched.kind, BodyKind::Solid);
        assert_eq!(stitched.shape.solid_count().unwrap(), 1);
        let replayed = crate::history::replay(&document.history).expect("stitch replay");
        assert!(
            replayed
                .bodies
                .iter()
                .any(|body| body.kind == BodyKind::Solid)
        );
    }

    #[test]
    fn thickened_planar_patch_has_area_times_thickness_volume() {
        let mut document = Document::new();
        let box_id = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let (face, _) = top_face(&document, box_id);
        let edges = face_edges(&document.bodies[0].shape, face);
        let patch = document.apply_patch(box_id, &edges).unwrap();
        assert!(document.apply_thicken(patch, 2.0));
        let body = document
            .bodies
            .iter()
            .find(|body| body.id == patch)
            .unwrap();
        assert_eq!(body.kind, BodyKind::Solid);
        let volume = body.shape.volume_properties().unwrap().volume;
        assert!((volume - 200.0).abs() < 5.0, "volume was {volume}");
    }

    #[test]
    fn delete_box_face_heals_or_rejects_without_mutation() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(10.0),
        });
        let before = document.bodies[0].shape.to_brep_data().unwrap();
        let history_len = document.history.len();
        if document.apply_delete_faces(body, &[0]) {
            let result = &document.bodies[0];
            assert_eq!(result.kind, BodyKind::Solid);
            assert!(result.shape.face_count().unwrap() >= 5);
            assert!(result.shape.check().unwrap().is_empty());
        } else {
            assert_eq!(document.history.len(), history_len);
            assert_eq!(document.bodies[0].shape.to_brep_data().unwrap(), before);
        }
    }

    #[test]
    fn native_roundtrip_preserves_surface_kind() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("surface-kind.f3d");
        let mut document = Document::new();
        let source = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        let edges = face_edges(&document.bodies[0].shape, 0);
        document.apply_patch(source, &edges).unwrap();
        document.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(loaded.bodies[1].kind, BodyKind::Surface);
    }

    #[test]
    fn cosmetic_thread_metadata_persists_in_f3d() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("thread.f3d");
        let mut document = Document::new();
        let id = document.add_body(
            "Cylinder",
            Shape::cylinder(DVec3::ZERO, 10.0, DVec3::Z, 20.0),
        );
        let face = (0..document.bodies[0].shape.face_count().unwrap())
            .find(|&index| {
                document.bodies[0].shape.face_surface_kind(index).ok()
                    == Some(occt::SurfaceKind::Cylinder)
            })
            .unwrap() as u32;
        assert!(document.apply_thread(id, face, true, ThreadMode::Cosmetic, 2.0, 20.0));
        document.save_to(&path).unwrap();
        let loaded = Document::load_from(&path).unwrap();
        assert_eq!(
            loaded.bodies[0].cosmetic_threads,
            document.bodies[0].cosmetic_threads
        );
    }

    #[test]
    fn modeled_external_thread_reduces_volume() {
        let mut document = Document::new();
        let id = document.add_body(
            "Cylinder",
            Shape::cylinder(DVec3::ZERO, 10.0, DVec3::Z, 12.0),
        );
        let before = document.bodies[0].shape.volume_properties().unwrap().volume;
        let face = (0..document.bodies[0].shape.face_count().unwrap())
            .find(|&index| {
                document.bodies[0].shape.face_surface_kind(index).ok()
                    == Some(occt::SurfaceKind::Cylinder)
            })
            .unwrap() as u32;
        assert!(document.apply_thread(id, face, true, ThreadMode::Modeled, 3.0, 12.0));
        let after = document.bodies[0].shape.volume_properties().unwrap().volume;
        assert!(after < before);
    }
}
