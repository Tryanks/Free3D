//! Pure parametric operations and deterministic document replay.

use std::path::PathBuf;
use std::{fmt, ops::Deref};

use glam::DVec3;
use occt::{Shape, SurfaceKind};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{
    assembly::{Joint, JointId},
    constraint::Constraint,
    document::{BodyId, BooleanOp, Document, DressUp, Material, ThreadMode, TransformOp},
    sketch::{SketchId, SketchItem, SketchPlane},
    tools::extrude::{ExtrudeDrag, ExtrudeMode, ExtrudeSideMode, ProfileExtrudeDrag, face_frame},
};

/// A replayable numeric feature parameter with optional source expression.
///
/// Legacy project files store these parameters as bare JSON numbers. Expression-backed
/// values use an object so the last evaluated value remains available for recovery.
#[derive(Clone, Debug, PartialEq)]
pub struct Num {
    /// Last successfully evaluated finite value.
    pub value: f64,
    /// Identifier-bearing source expression, when the value is variable-driven.
    pub expr: Option<String>,
}

impl Num {
    /// Creates a literal parameter without expression storage.
    pub const fn literal(value: f64) -> Self {
        Self { value, expr: None }
    }

    /// Creates a parameter and retains only identifier-bearing expressions.
    pub fn from_input(value: f64, expression: String) -> Self {
        Self {
            value,
            expr: crate::ui::expr::contains_identifier(&expression).then_some(expression),
        }
    }

    /// Text used to pre-fill an editor.
    pub fn editor_text(&self) -> String {
        self.expr
            .clone()
            .unwrap_or_else(|| format!("{:.3}", self.value))
    }
}

impl From<f64> for Num {
    fn from(value: f64) -> Self {
        Self::literal(value)
    }
}

impl Deref for Num {
    type Target = f64;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl fmt::Display for Num {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(expression) = &self.expr {
            formatter.write_str(expression)
        } else {
            self.value.fmt(formatter)
        }
    }
}

impl Serialize for Num {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.expr.is_none() {
            return serializer.serialize_f64(self.value);
        }
        #[derive(Serialize)]
        struct ExpressionNum<'a> {
            value: f64,
            expr: &'a str,
        }
        ExpressionNum {
            value: self.value,
            expr: self.expr.as_deref().expect("checked expression"),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Num {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Literal(f64),
            Expression { value: f64, expr: String },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Literal(value) => Self::literal(value),
            Repr::Expression { value, expr } => Self {
                value,
                expr: Some(expr),
            },
        })
    }
}

/// Serializable subset of OCCT surface classifications used by face fingerprints.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum SurfaceKindTag {
    /// Planar surface.
    Plane,
    /// Cylindrical surface.
    Cylinder,
    /// Spherical surface.
    Sphere,
    /// Conical surface.
    Cone,
    /// Toroidal surface.
    Torus,
    /// Bezier surface.
    Bezier,
    /// B-spline surface.
    BSpline,
    /// Any other OCCT surface type.
    Other,
}

impl From<SurfaceKind> for SurfaceKindTag {
    fn from(kind: SurfaceKind) -> Self {
        match kind {
            SurfaceKind::Plane => Self::Plane,
            SurfaceKind::Cylinder => Self::Cylinder,
            SurfaceKind::Sphere => Self::Sphere,
            SurfaceKind::Cone => Self::Cone,
            SurfaceKind::Torus => Self::Torus,
            SurfaceKind::Bezier => Self::Bezier,
            SurfaceKind::BSpline => Self::BSpline,
            SurfaceKind::Other => Self::Other,
        }
    }
}

/// Geometric signature used when a face's explorer index changes during replay.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct FacePrint {
    /// Underlying OCCT surface classification.
    pub kind: SurfaceKindTag,
    /// Surface center of mass in world coordinates.
    pub center: DVec3,
    /// Trimmed face area.
    pub area: f64,
    /// Outward normal recorded only for planar faces.
    pub normal_hint: Option<DVec3>,
}

/// Persisted face reference with legacy explorer-index compatibility.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceRef {
    /// Recorded OCCT explorer index used by the fast path and legacy files.
    pub index: u32,
    /// Optional geometric fallback signature.
    pub print: Option<FacePrint>,
}

impl From<u32> for FaceRef {
    fn from(index: u32) -> Self {
        Self { index, print: None }
    }
}

impl Serialize for FaceRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.print.is_none() {
            return serializer.serialize_u32(self.index);
        }
        #[derive(Serialize)]
        struct Fingerprinted<'a> {
            index: u32,
            print: &'a FacePrint,
        }
        Fingerprinted {
            index: self.index,
            print: self.print.as_ref().expect("checked face print"),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FaceRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Legacy(u32),
            Fingerprinted { index: u32, print: FacePrint },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Legacy(index) => index.into(),
            Repr::Fingerprinted { index, print } => Self {
                index,
                print: Some(print),
            },
        })
    }
}

/// Geometric signature used when an edge's explorer index changes during replay.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct EdgePrint {
    /// Point halfway along the sampled curve length.
    pub midpoint: DVec3,
    /// Exact OCCT curve length.
    pub length: f64,
}

/// Persisted edge reference with legacy explorer-index compatibility.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeRef {
    /// Recorded OCCT explorer index used by the fast path and legacy files.
    pub index: u32,
    /// Optional geometric fallback signature.
    pub print: Option<EdgePrint>,
}

impl From<u32> for EdgeRef {
    fn from(index: u32) -> Self {
        Self { index, print: None }
    }
}

impl Serialize for EdgeRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        if self.print.is_none() {
            return serializer.serialize_u32(self.index);
        }
        #[derive(Serialize)]
        struct Fingerprinted<'a> {
            index: u32,
            print: &'a EdgePrint,
        }
        Fingerprinted {
            index: self.index,
            print: self.print.as_ref().expect("checked edge print"),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EdgeRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            Legacy(u32),
            Fingerprinted { index: u32, print: EdgePrint },
        }
        Ok(match Repr::deserialize(deserializer)? {
            Repr::Legacy(index) => index.into(),
            Repr::Fingerprinted { index, print } => Self {
                index,
                print: Some(print),
            },
        })
    }
}

fn edge_midpoint(shape: &Shape, index: usize) -> Option<DVec3> {
    let (minimum, maximum) = shape.aabb().ok()?;
    let deflection = ((maximum - minimum).length() * 1.0e-4).max(1.0e-7);
    let points = shape.edge_polyline(index, deflection).ok()?;
    let total: f64 = points
        .windows(2)
        .map(|pair| pair[0].distance(pair[1]))
        .sum();
    if total <= f64::EPSILON {
        return points.first().copied();
    }
    let mut traversed = 0.0;
    for pair in points.windows(2) {
        let segment = pair[0].distance(pair[1]);
        if traversed + segment >= total * 0.5 {
            return Some(pair[0].lerp(pair[1], (total * 0.5 - traversed) / segment));
        }
        traversed += segment;
    }
    points.last().copied()
}

/// Captures a replayable face reference from the operation's current source shape.
pub(crate) fn face_ref(shape: &Shape, index: u32) -> FaceRef {
    let print = (|| {
        let kind = SurfaceKindTag::from(shape.face_surface_kind(index as usize).ok()?);
        let center = shape.face_center_of_mass(index as usize).ok()?;
        let area = shape.face_area(index as usize).ok()?;
        let normal_hint = (kind == SurfaceKindTag::Plane)
            .then(|| shape.face_normal_at(index as usize, center).ok())
            .flatten();
        Some(FacePrint {
            kind,
            center,
            area,
            normal_hint,
        })
    })();
    FaceRef { index, print }
}

/// Captures a replayable edge reference from the operation's current source shape.
pub(crate) fn edge_ref(shape: &Shape, index: u32) -> EdgeRef {
    let print = (|| {
        Some(EdgePrint {
            midpoint: edge_midpoint(shape, index as usize)?,
            length: shape.edge_length(index as usize).ok()?,
        })
    })();
    EdgeRef { index, print }
}

/// Replayable reference to a sweep trajectory.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum PathRef {
    /// The closed outline of another detected sketch profile.
    Profile {
        sketch: SketchId,
        profile_index: usize,
    },
    /// A connected open chain of creation-ordered sketch line entities.
    OpenChain {
        sketch: SketchId,
        entity_indices: Vec<usize>,
    },
}

/// Parameters for a built-in primitive, including its world placement.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum PrimitiveKind {
    /// Axis-aligned box between two corners.
    Box { min: DVec3, max: DVec3 },
    /// Cylinder based at `origin` and extending along `axis`.
    Cylinder {
        origin: DVec3,
        radius: f64,
        axis: DVec3,
        height: f64,
    },
    /// Sphere with a world-space center.
    Sphere { center: DVec3, radius: f64 },
    /// World-Z cone based at `origin`.
    Cone {
        origin: DVec3,
        bottom_radius: f64,
        height: f64,
    },
    /// World-Z torus centered at `center`.
    Torus {
        center: DVec3,
        major_radius: f64,
        minor_radius: f64,
    },
    /// Axis-aligned ellipsoid.
    Ellipsoid { center: DVec3, radii: DVec3 },
    /// Regular world-Z polygonal prism.
    Prism {
        center: DVec3,
        radius: f64,
        sides: u32,
        height: f64,
    },
    /// World-Z wedge.
    Wedge {
        origin: DVec3,
        dx: f64,
        dy: f64,
        dz: f64,
        top_dx: f64,
    },
}

impl PrimitiveKind {
    /// Builds this primitive through the shared OCCT constructors.
    pub fn shape(self) -> Shape {
        match self {
            Self::Box { min, max } => Shape::box_from_corners(min, max),
            Self::Cylinder {
                origin,
                radius,
                axis,
                height,
            } => Shape::cylinder(origin, radius, axis, height),
            Self::Sphere { center, radius } => Shape::sphere(center, radius),
            Self::Cone {
                origin,
                bottom_radius,
                height,
            } => Shape::cone(origin, bottom_radius, height),
            Self::Torus {
                center,
                major_radius,
                minor_radius,
            } => Shape::torus(center, major_radius, minor_radius),
            Self::Ellipsoid { center, radii } => Shape::ellipsoid(center, radii),
            Self::Prism {
                center,
                radius,
                sides,
                height,
            } => Shape::regular_prism(center, radius, sides, height),
            Self::Wedge {
                origin,
                dx,
                dy,
                dz,
                top_dx,
            } => Shape::wedge(origin, dx, dy, dz, top_dx),
        }
        .expect("valid built-in primitive parameters")
    }

    /// Stable display stem used for automatic body naming.
    pub fn name(self) -> &'static str {
        match self {
            Self::Box { .. } => crate::i18n::t("Box"),
            Self::Cylinder { .. } => crate::i18n::t("Cylinder"),
            Self::Sphere { .. } => crate::i18n::t("Sphere"),
            Self::Cone { .. } => crate::i18n::t("Cone"),
            Self::Torus { .. } => crate::i18n::t("Torus"),
            Self::Ellipsoid { .. } => crate::i18n::t("Ellipsoid"),
            Self::Prism { .. } => crate::i18n::t("Prism"),
            Self::Wedge { .. } => crate::i18n::t("Wedge"),
        }
    }
}

/// Axial extent of a parametric hole.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum HoleKind {
    /// Pierces the complete body along the selected face's inward normal.
    Through,
    /// Stops at the requested depth.
    Blind { depth: Num },
}

/// Optional entrance treatment of a parametric hole.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum HoleCut {
    /// Straight bore only.
    None,
    /// Cylindrical stepped recess.
    Counterbore { diameter: Num, depth: Num },
    /// Conical entrance recess with included angle in degrees.
    Countersink { diameter: Num, angle_degrees: Num },
}

/// One replayable document mutation containing no OCCT handles.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "t", content = "v")]
pub enum HistoryOp {
    /// Add a built-in primitive.
    AddPrimitive { kind: PrimitiveKind },
    /// Add an empty sketch.
    AddSketch {
        plane: SketchPlane,
        #[serde(default)]
        support_body: Option<BodyId>,
    },
    /// Add a bounded construction plane.
    AddConstructionPlane { plane: SketchPlane },
    /// Add a construction axis.
    AddConstructionAxis { origin: DVec3, direction: DVec3 },
    /// Add a construction point.
    AddConstructionPoint { position: DVec3 },
    /// Add a portable planar raster reference.
    AddReferenceImage {
        name: String,
        data: String,
        width_mm: f64,
        plane: SketchPlane,
        origin: glam::DVec2,
    },
    /// Replace a sketch's complete editable state.
    SketchState {
        sketch: SketchId,
        entities: Vec<SketchItem>,
        constraints: Vec<Constraint>,
        pinned: Vec<usize>,
    },
    /// Extrude either a sketch profile or a body face.
    Extrude {
        sketch: Option<SketchId>,
        profile_index: Option<usize>,
        body: Option<BodyId>,
        face_index: Option<FaceRef>,
        distance: Num,
        opposite_distance: Num,
        side_mode: ExtrudeSideMode,
        mode: ExtrudeMode,
    },
    /// Extrude an open sketch chain into a surface body.
    SurfaceExtrude {
        sketch: SketchId,
        entity_indices: Vec<usize>,
        distance: Num,
        opposite_distance: Num,
        side_mode: ExtrudeSideMode,
    },
    /// Offset a planar body face.
    OffsetFace {
        body: BodyId,
        face_index: FaceRef,
        distance: Num,
    },
    /// Offset several selected faces as one transaction.
    OffsetFaces {
        faces: Vec<(BodyId, FaceRef)>,
        distance: Num,
    },
    /// Extend or trim a planar face to a parallel target plane.
    ReplaceFace {
        body: BodyId,
        face_index: FaceRef,
        target_origin: DVec3,
        target_normal: DVec3,
    },
    /// Apply a multi-body boolean.
    Boolean {
        op: BooleanOp,
        target: BodyId,
        tools: Vec<BodyId>,
    },
    /// Round selected body edges.
    Fillet {
        body: BodyId,
        edges: Vec<EdgeRef>,
        radius: Num,
        #[serde(default)]
        end_radius: Option<Num>,
    },
    /// Bevel selected body edges.
    Chamfer {
        body: BodyId,
        edges: Vec<EdgeRef>,
        radius: Num,
    },
    /// Hollow a body through selected opening faces.
    Shell {
        body: BodyId,
        faces: Vec<FaceRef>,
        thickness: Num,
    },
    /// Cut a parametric hole normal to a planar body face.
    Hole {
        body: BodyId,
        face_index: FaceRef,
        at: DVec3,
        diameter: Num,
        kind: HoleKind,
        cut: HoleCut,
    },
    /// Draft body faces relative to a neutral plane.
    Draft {
        body: BodyId,
        face_indices: Vec<FaceRef>,
        direction: DVec3,
        neutral_origin: DVec3,
        neutral_normal: DVec3,
        angle_degrees: Num,
    },
    /// Apply a rigid body transform.
    Transform { ids: Vec<BodyId>, op: TransformOp },
    /// Keep sources and create transformed copies at successive multiples.
    MultiTransform {
        ids: Vec<BodyId>,
        op: TransformOp,
        count: u32,
    },
    /// Uniformly scale bodies about a world-space pivot.
    Scale {
        ids: Vec<BodyId>,
        factor: Num,
        pivot: DVec3,
    },
    /// Split one body at a world-Y plane.
    Split { body: BodyId, y: f64 },
    /// Revolve a sketch profile around an explicit world-space axis.
    Revolve {
        sketch: SketchId,
        profile_index: usize,
        axis_origin: DVec3,
        axis_direction: DVec3,
        angle_degrees: Num,
        #[serde(default)]
        mode: ExtrudeMode,
    },
    /// Revolve an open sketch chain into a surface body.
    SurfaceRevolve {
        sketch: SketchId,
        entity_indices: Vec<usize>,
        axis_origin: DVec3,
        axis_direction: DVec3,
        angle_degrees: Num,
    },
    /// Fill a closed edge loop as a new surface body.
    Patch { body: BodyId, edges: Vec<EdgeRef> },
    /// Sew surface bodies, promoting a closed shell to a solid.
    Stitch { bodies: Vec<BodyId> },
    /// Thicken a surface body into a solid.
    Thicken { body: BodyId, thickness: Num },
    /// Delete solid faces and heal the surrounding topology.
    DeleteFace { body: BodyId, faces: Vec<FaceRef> },
    /// Sweep one closed section along a closed or open sketch path.
    Sweep {
        sketch: SketchId,
        profile_index: usize,
        path: PathRef,
    },
    /// Loft through closed sketch sections in selection order.
    Loft { sections: Vec<(SketchId, usize)> },
    /// Sweep a circular profile along a cylindrical helix.
    Helix {
        origin: DVec3,
        axis: DVec3,
        radius: Num,
        pitch: Num,
        turns: Num,
        profile_radius: Num,
        left_handed: bool,
    },
    /// Cosmetic or modeled thread applied to a cylindrical face.
    Thread {
        body: BodyId,
        face_index: FaceRef,
        external: bool,
        mode: ThreadMode,
        pitch: Num,
        depth: Num,
    },
    /// Mirror bodies across an explicit world-space plane.
    Mirror {
        ids: Vec<BodyId>,
        plane_origin: DVec3,
        plane_normal: DVec3,
    },
    /// Pattern bodies along an explicit world-space direction.
    LinearPattern {
        ids: Vec<BodyId>,
        axis_origin: DVec3,
        axis_direction: DVec3,
        spacing: Num,
        count: u32,
    },
    /// Pattern bodies around an explicit world-space axis.
    CircularPattern {
        ids: Vec<BodyId>,
        axis_origin: DVec3,
        axis_direction: DVec3,
        count: u32,
    },
    /// Import one external geometry or sketch file.
    #[serde(alias = "ImportStep")]
    ImportFile { path: PathBuf },
    /// Delete bodies.
    DeleteBodies { ids: Vec<BodyId> },
    /// Change body visibility; this is non-geometric.
    SetVisible { id: BodyId, visible: bool },
    /// Rename a body; this is non-geometric.
    Rename { id: BodyId, name: String },
    /// Change a body's persisted viewport material.
    SetMaterial { body: BodyId, material: Material },
    /// Add a same-document assembly joint.
    AddJoint { joint: Joint },
    /// Delete an assembly joint.
    DeleteJoint { id: JointId },
    /// Drive an assembly joint.
    SetJointValue {
        id: JointId,
        value: f64,
        value2: f64,
    },
    /// Toggle a body's grounded anchor state.
    SetGrounded { body: BodyId, grounded: bool },
}

impl HistoryOp {
    /// Short operation label for the History panel.
    pub fn label(&self) -> &'static str {
        match self {
            Self::AddPrimitive { kind } => kind.name(),
            Self::AddSketch { .. } => "Sketch",
            Self::AddConstructionPlane { .. } => "Construction Plane",
            Self::AddConstructionAxis { .. } => "Construction Axis",
            Self::AddConstructionPoint { .. } => "Construction Point",
            Self::AddReferenceImage { .. } => "Reference Image",
            Self::SketchState { .. } => "Sketch edit",
            Self::Extrude { .. } => "Extrude",
            Self::SurfaceExtrude { .. } => "Surface Extrude",
            Self::OffsetFace { .. } => "Offset Face",
            Self::OffsetFaces { .. } => "Offset Face",
            Self::ReplaceFace { .. } => "Replace Face",
            Self::Boolean { op, .. } => match op {
                BooleanOp::Union => "Union",
                BooleanOp::Subtract => "Subtract",
                BooleanOp::Intersect => "Intersect",
            },
            Self::Fillet { .. } => "Fillet",
            Self::Chamfer { .. } => "Chamfer",
            Self::Shell { .. } => "Shell",
            Self::Hole { .. } => "Hole",
            Self::Draft { .. } => "Draft",
            Self::Transform { .. } => "Transform",
            Self::MultiTransform { .. } => "Multi Transform",
            Self::Scale { .. } => "Scale",
            Self::Split { .. } => "Split Body",
            Self::Revolve { .. } => "Revolve",
            Self::SurfaceRevolve { .. } => "Surface Revolve",
            Self::Patch { .. } => "Patch",
            Self::Stitch { .. } => "Stitch",
            Self::Thicken { .. } => "Thicken",
            Self::DeleteFace { .. } => "Delete Face",
            Self::Sweep { .. } => "Sweep",
            Self::Loft { .. } => "Loft",
            Self::Helix { .. } => "Helix",
            Self::Thread { .. } => "Thread",
            Self::Mirror { .. } => "Mirror",
            Self::LinearPattern { .. } => "Linear Pattern",
            Self::CircularPattern { .. } => "Circular Pattern",
            Self::ImportFile { .. } => "Import File",
            Self::DeleteBodies { .. } => "Delete",
            Self::SetVisible { .. } => "Visibility",
            Self::Rename { .. } => "Rename",
            Self::SetMaterial { .. } => "Material",
            Self::AddJoint { .. } => "Joint",
            Self::DeleteJoint { .. } => "Delete Joint",
            Self::SetJointValue { .. } => "Drive Joint",
            Self::SetGrounded { .. } => "Ground",
        }
    }

    /// Existing tool icon used for this operation kind.
    pub const fn icon(&self) -> &'static str {
        match self {
            Self::AddPrimitive { kind } => match kind {
                PrimitiveKind::Box { .. } => "box",
                PrimitiveKind::Cylinder { .. } => "cylinder",
                PrimitiveKind::Sphere { .. } => "sphere",
                PrimitiveKind::Cone { .. } => "cone",
                PrimitiveKind::Torus { .. } => "torus",
                PrimitiveKind::Ellipsoid { .. } => "sphere",
                PrimitiveKind::Prism { .. } => "polygon",
                PrimitiveKind::Wedge { .. } => "box",
            },
            Self::AddSketch { .. } | Self::SketchState { .. } => "sketch",
            Self::AddConstructionPlane { .. } => "plane",
            Self::AddConstructionAxis { .. } => "axis",
            Self::AddConstructionPoint { .. } => "point",
            Self::AddReferenceImage { .. } => "image",
            Self::Extrude { .. } | Self::SurfaceExtrude { .. } => "extrude",
            Self::OffsetFace { .. } | Self::OffsetFaces { .. } => "offset",
            Self::ReplaceFace { .. } => "offset",
            Self::Boolean { op, .. } => match op {
                BooleanOp::Union => "union",
                BooleanOp::Subtract => "subtract",
                BooleanOp::Intersect => "intersect",
            },
            Self::Fillet { .. } | Self::Chamfer { .. } => "fillet",
            Self::Shell { .. } => "shell",
            Self::Hole { .. } => "hole",
            Self::Draft { .. } => "offset",
            Self::Transform { .. } => "move",
            Self::MultiTransform { .. } => "move",
            Self::Scale { .. } => "scale",
            Self::Split { .. } => "split",
            Self::Revolve { .. } | Self::SurfaceRevolve { .. } => "revolve",
            Self::Patch { .. } => "patch",
            Self::Stitch { .. } => "stitch",
            Self::Thicken { .. } => "thicken",
            Self::DeleteFace { .. } => "delete-face",
            Self::Sweep { .. } => "sweep",
            Self::Loft { .. } => "loft",
            Self::Helix { .. } => "sweep",
            Self::Thread { .. } => "sweep",
            Self::Mirror { .. } => "mirror",
            Self::LinearPattern { .. } | Self::CircularPattern { .. } => "pattern",
            Self::ImportFile { .. } => "import",
            Self::DeleteBodies { .. } => "trash",
            Self::SetVisible { .. } => "eye",
            Self::Rename { .. } => "items",
            Self::SetMaterial { .. } => "display",
            Self::AddJoint { .. } | Self::SetJointValue { .. } => "move",
            Self::DeleteJoint { .. } => "trash",
            Self::SetGrounded { .. } => "axis",
        }
    }

    /// One-line parameter summary for the History panel.
    pub fn summary(&self) -> String {
        match self {
            Self::AddPrimitive {
                kind: PrimitiveKind::Box { min, max },
            } => {
                let size = *max - *min;
                format!("{:.0}×{:.0}×{:.0}", size.x, size.y, size.z)
            }
            Self::AddPrimitive {
                kind: PrimitiveKind::Cylinder { radius, height, .. },
            } => format!("r {radius:.1} × {height:.1}"),
            Self::AddPrimitive {
                kind: PrimitiveKind::Sphere { radius, .. },
            } => format!("r {radius:.1}"),
            Self::AddPrimitive {
                kind:
                    PrimitiveKind::Cone {
                        bottom_radius,
                        height,
                        ..
                    },
            } => format!("r {bottom_radius:.1} × {height:.1}"),
            Self::AddPrimitive {
                kind:
                    PrimitiveKind::Torus {
                        major_radius,
                        minor_radius,
                        ..
                    },
            } => format!("{major_radius:.1} / {minor_radius:.1}"),
            Self::AddPrimitive {
                kind: PrimitiveKind::Ellipsoid { radii, .. },
            } => {
                format!(
                    "{:.1}×{:.1}×{:.1}",
                    radii.x * 2.0,
                    radii.y * 2.0,
                    radii.z * 2.0
                )
            }
            Self::AddPrimitive {
                kind:
                    PrimitiveKind::Prism {
                        sides,
                        radius,
                        height,
                        ..
                    },
            } => {
                format!("{sides} sides · r {radius:.1} × {height:.1}")
            }
            Self::AddPrimitive {
                kind: PrimitiveKind::Wedge { dx, dy, dz, .. },
            } => {
                format!("{dx:.1}×{dy:.1}×{dz:.1}")
            }
            Self::AddSketch { .. } => "empty".to_owned(),
            Self::AddConstructionPlane { plane } => {
                format!("offset {:.1}", plane.origin.dot(plane.normal()))
            }
            Self::AddConstructionAxis { .. } => "origin + direction".to_owned(),
            Self::AddConstructionPoint { position } => {
                format!("{:.1}, {:.1}, {:.1}", position.x, position.y, position.z)
            }
            Self::AddReferenceImage { width_mm, .. } => format!("width {width_mm:.1} mm"),
            Self::SketchState { entities, .. } => format!("{} entities", entities.len()),
            Self::Extrude {
                distance,
                opposite_distance,
                side_mode: ExtrudeSideMode::TwoSided,
                ..
            } => format!("A {distance} / B {opposite_distance}"),
            Self::Extrude {
                distance,
                side_mode: ExtrudeSideMode::Symmetric,
                ..
            } => distance.expr.as_ref().map_or_else(
                || format!("±{:.1}", distance.abs() * 0.5),
                |expression| format!("±{expression}"),
            ),
            Self::Extrude { distance, .. }
            | Self::OffsetFace { distance, .. }
            | Self::OffsetFaces { distance, .. } => {
                format!("{distance:.1}")
            }
            Self::SurfaceExtrude {
                distance,
                entity_indices,
                ..
            } => format!("{distance:.1} · {} edges", entity_indices.len()),
            Self::ReplaceFace { .. } => "parallel plane".to_owned(),
            Self::Boolean { tools, .. } => format!("{} tools", tools.len()),
            Self::Fillet {
                edges,
                radius,
                end_radius,
                ..
            } => end_radius.as_ref().map_or_else(
                || format!("r {radius:.1} ({} edges)", edges.len()),
                |end| format!("r {radius:.1} → {end:.1} ({} edges)", edges.len()),
            ),
            Self::Chamfer { edges, radius, .. } => {
                format!("r {radius:.1} ({} edges)", edges.len())
            }
            Self::Shell { thickness, .. } => format!("{thickness:.1}"),
            Self::Hole { diameter, kind, .. } => match kind {
                HoleKind::Through => format!("⌀{diameter:.1} through"),
                HoleKind::Blind { depth } => format!("⌀{diameter:.1} × {depth:.1}"),
            },
            Self::Draft {
                face_indices,
                angle_degrees,
                ..
            } => {
                format!("{} faces · {angle_degrees:.1}°", face_indices.len())
            }
            Self::Transform { ids, .. } | Self::Mirror { ids, .. } => {
                format!("{} bodies", ids.len())
            }
            Self::MultiTransform { ids, count, .. } => {
                format!("{} bodies ×{count}", ids.len())
            }
            Self::Scale { ids, factor, .. } => format!("{} bodies ×{factor:.2}", ids.len()),
            Self::Split { y, .. } => format!("Y {y:.1}"),
            Self::Revolve { angle_degrees, .. } => format!("{angle_degrees:.1}°"),
            Self::SurfaceRevolve {
                angle_degrees,
                entity_indices,
                ..
            } => format!("{angle_degrees:.1}° · {} edges", entity_indices.len()),
            Self::Patch { edges, .. } => format!("{} edges", edges.len()),
            Self::Stitch { bodies } => format!("{} surfaces", bodies.len()),
            Self::Thicken { thickness, .. } => format!("{thickness:.1}"),
            Self::DeleteFace { faces, .. } => format!("{} faces", faces.len()),
            Self::Sweep { path, .. } => match path {
                PathRef::Profile { .. } => "closed path".to_owned(),
                PathRef::OpenChain { entity_indices, .. } => {
                    format!("{} path edges", entity_indices.len())
                }
            },
            Self::Loft { sections } => format!("{} sections", sections.len()),
            Self::Helix {
                radius,
                pitch,
                turns,
                profile_radius,
                left_handed,
                ..
            } => format!(
                "r {radius:.1} · p {pitch:.1} · {turns:.1} turns · wire {profile_radius:.1} · {}",
                if *left_handed { "left" } else { "right" }
            ),
            Self::Thread {
                external,
                mode,
                pitch,
                depth,
                ..
            } => format!(
                "{} · {} · p {pitch:.1} · d {depth:.1}",
                if *external { "external" } else { "internal" },
                if *mode == ThreadMode::Cosmetic {
                    "cosmetic"
                } else {
                    "modeled"
                },
            ),
            Self::LinearPattern { spacing, count, .. } => {
                format!("{count} × {spacing:.1}")
            }
            Self::CircularPattern { count, .. } => format!("{count} copies"),
            Self::ImportFile { path } => path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("STEP")
                .to_owned(),
            Self::DeleteBodies { ids } => format!("{} bodies", ids.len()),
            Self::SetVisible { visible, .. } => {
                if *visible { "shown" } else { "hidden" }.to_owned()
            }
            Self::Rename { name, .. } => name.clone(),
            Self::SetMaterial { material, .. } => format!(
                "metal {:.0}% · rough {:.0}%",
                material.metallic * 100.0,
                material.roughness * 100.0
            ),
            Self::AddJoint { joint } => format!("{} · {:?}", joint.name, joint.kind),
            Self::DeleteJoint { id } => format!("#{}", id.0),
            Self::SetJointValue { value, value2, .. } => format!("{value:.3} / {value2:.3}"),
            Self::SetGrounded { grounded, .. } => {
                if *grounded { "grounded" } else { "free" }.to_owned()
            }
        }
    }

    /// Primary editable numeric value, if this operation has one.
    pub fn numeric_value(&self) -> Option<f64> {
        match self {
            Self::Extrude { distance, .. }
            | Self::SurfaceExtrude { distance, .. }
            | Self::OffsetFace { distance, .. }
            | Self::OffsetFaces { distance, .. } => Some(distance.value),
            Self::Fillet { radius, .. } | Self::Chamfer { radius, .. } => Some(radius.value),
            Self::Shell { thickness, .. } => Some(thickness.value),
            Self::Hole { diameter, .. } => Some(diameter.value),
            Self::Draft { angle_degrees, .. } => Some(angle_degrees.value),
            Self::Revolve { angle_degrees, .. } => Some(angle_degrees.value),
            Self::SurfaceRevolve { angle_degrees, .. } => Some(angle_degrees.value),
            Self::Thicken { thickness, .. } => Some(thickness.value),
            Self::LinearPattern { spacing, .. } => Some(spacing.value),
            Self::Scale { factor, .. } => Some(factor.value),
            Self::CircularPattern { count, .. } => Some(f64::from(*count)),
            Self::AddReferenceImage { width_mm, .. } => Some(*width_mm),
            _ => None,
        }
    }

    /// Primary editor text, preserving an expression when present.
    pub fn numeric_editor_text(&self) -> Option<String> {
        match self {
            Self::Extrude { distance, .. }
            | Self::SurfaceExtrude { distance, .. }
            | Self::OffsetFace { distance, .. }
            | Self::OffsetFaces { distance, .. } => Some(distance.editor_text()),
            Self::Fillet { radius, .. } | Self::Chamfer { radius, .. } => {
                Some(radius.editor_text())
            }
            Self::Shell { thickness, .. } => Some(thickness.editor_text()),
            Self::Hole { diameter, .. } => Some(diameter.editor_text()),
            Self::Draft { angle_degrees, .. } | Self::Revolve { angle_degrees, .. } => {
                Some(angle_degrees.editor_text())
            }
            Self::SurfaceRevolve { angle_degrees, .. } => Some(angle_degrees.editor_text()),
            Self::Thicken { thickness, .. } => Some(thickness.editor_text()),
            Self::LinearPattern { spacing, .. } => Some(spacing.editor_text()),
            Self::Scale { factor, .. } => Some(factor.editor_text()),
            _ => self.numeric_value().map(|value| format!("{value:.3}")),
        }
    }

    /// Updates the primary editable numeric value.
    pub fn set_numeric_input(&mut self, value: f64, expression: String) -> bool {
        if !value.is_finite() {
            return false;
        }
        match self {
            Self::Extrude { distance, .. }
            | Self::SurfaceExtrude { distance, .. }
            | Self::OffsetFace { distance, .. }
            | Self::OffsetFaces { distance, .. } => *distance = Num::from_input(value, expression),
            Self::Fillet { radius, .. } | Self::Chamfer { radius, .. } => {
                *radius = Num::from_input(value, expression)
            }
            Self::Shell { thickness, .. } => *thickness = Num::from_input(value, expression),
            Self::Hole { diameter, .. } => *diameter = Num::from_input(value, expression),
            Self::Draft { angle_degrees, .. } | Self::Revolve { angle_degrees, .. } => {
                *angle_degrees = Num::from_input(value, expression)
            }
            Self::SurfaceRevolve { angle_degrees, .. } => {
                *angle_degrees = Num::from_input(value, expression)
            }
            Self::Thicken { thickness, .. } => *thickness = Num::from_input(value, expression),
            Self::LinearPattern { spacing, .. } => *spacing = Num::from_input(value, expression),
            Self::Scale { factor, .. } => *factor = Num::from_input(value, expression),
            Self::CircularPattern { count, .. } => *count = value.round() as u32,
            Self::AddReferenceImage { width_mm, .. } if value > 0.0 => *width_mm = value,
            _ => return false,
        }
        true
    }

    /// Updates the primary value as a literal, clearing any stored expression.
    #[cfg(test)]
    pub fn set_numeric_value(&mut self, value: f64) -> bool {
        self.set_numeric_input(value, String::new())
    }

    /// Visits every expression-capable numeric parameter in this operation.
    pub fn for_each_num_mut(&mut self, mut visit: impl FnMut(&mut Num)) {
        match self {
            Self::Extrude {
                distance,
                opposite_distance,
                ..
            } => {
                visit(distance);
                visit(opposite_distance);
            }
            Self::SurfaceExtrude {
                distance,
                opposite_distance,
                ..
            } => {
                visit(distance);
                visit(opposite_distance);
            }
            Self::OffsetFace { distance, .. } | Self::OffsetFaces { distance, .. } => {
                visit(distance)
            }
            Self::Fillet {
                radius, end_radius, ..
            } => {
                visit(radius);
                if let Some(end_radius) = end_radius {
                    visit(end_radius);
                }
            }
            Self::Chamfer { radius, .. } => visit(radius),
            Self::Shell { thickness, .. } => visit(thickness),
            Self::Hole {
                diameter,
                kind,
                cut,
                ..
            } => {
                visit(diameter);
                if let HoleKind::Blind { depth } = kind {
                    visit(depth);
                }
                match cut {
                    HoleCut::Counterbore { diameter, depth } => {
                        visit(diameter);
                        visit(depth);
                    }
                    HoleCut::Countersink {
                        diameter,
                        angle_degrees,
                    } => {
                        visit(diameter);
                        visit(angle_degrees);
                    }
                    HoleCut::None => {}
                }
            }
            Self::Draft { angle_degrees, .. } | Self::Revolve { angle_degrees, .. } => {
                visit(angle_degrees)
            }
            Self::SurfaceRevolve { angle_degrees, .. } => visit(angle_degrees),
            Self::Thicken { thickness, .. } => visit(thickness),
            Self::Scale { factor, .. } => visit(factor),
            Self::Helix {
                radius,
                pitch,
                turns,
                profile_radius,
                ..
            } => {
                visit(radius);
                visit(pitch);
                visit(turns);
                visit(profile_radius);
            }
            Self::Thread { pitch, depth, .. } => {
                visit(pitch);
                visit(depth);
            }
            Self::LinearPattern { spacing, .. } => visit(spacing),
            _ => {}
        }
    }

    /// Optional editable count for a linear pattern.
    pub fn set_secondary_count(&mut self, value: f64) -> bool {
        if let Self::LinearPattern { count, .. } = self
            && value.is_finite()
        {
            *count = value.round() as u32;
            true
        } else {
            false
        }
    }
}

/// A timeline entry and its suppression state.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct HistoryStep {
    /// Operation parameters.
    pub op: HistoryOp,
    /// Suppressed steps are retained but skipped during replay.
    pub suppressed: bool,
}

impl HistoryStep {
    /// Creates an enabled history step.
    pub const fn new(op: HistoryOp) -> Self {
        Self {
            op,
            suppressed: false,
        }
    }
}

/// A replay failure attributed to the first invalid history step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayError {
    /// Zero-based index of the failing step.
    pub step_index: usize,
    /// Human-readable failure detail.
    pub message: String,
}

fn failed(step_index: usize, message: impl Into<String>) -> ReplayError {
    ReplayError {
        step_index,
        message: message.into(),
    }
}

fn shape_diagonal(shape: &Shape) -> Result<f64, String> {
    let (minimum, maximum) = shape.aabb().map_err(|error| error.to_string())?;
    Ok((maximum - minimum).length().max(1.0e-9))
}

fn ratio(value: f64, reference: f64) -> f64 {
    if reference.abs() <= f64::EPSILON {
        if value.abs() <= f64::EPSILON {
            1.0
        } else {
            f64::INFINITY
        }
    } else {
        value / reference
    }
}

fn face_matches(print: &FacePrint, candidate: &FacePrint, diagonal: f64, relaxed: bool) -> bool {
    let center_tolerance = diagonal * if relaxed { 1.0e-2 } else { 1.0e-4 };
    let area_ratio = ratio(candidate.area, print.area);
    let (minimum_area, maximum_area) = if relaxed { (0.9, 1.1) } else { (0.99, 1.01) };
    candidate.kind == print.kind
        && candidate.center.distance(print.center) < center_tolerance
        && (minimum_area..=maximum_area).contains(&area_ratio)
}

fn face_print(shape: &Shape, index: u32) -> Option<FacePrint> {
    face_ref(shape, index).print
}

/// Resolves a persisted face reference against the current replay shape.
pub(crate) fn resolve_face(shape: &Shape, reference: &FaceRef) -> Result<u32, String> {
    let Some(print) = &reference.print else {
        return Ok(reference.index);
    };
    let diagonal = shape_diagonal(shape)?;
    if let Some(candidate) = face_print(shape, reference.index)
        && (face_matches(print, &candidate, diagonal, false)
            || face_matches(print, &candidate, diagonal, true))
    {
        return Ok(reference.index);
    }
    let face_count = shape.face_count().map_err(|error| error.to_string())?;
    let mut candidates = (0..face_count)
        .filter_map(|index| {
            let candidate = face_print(shape, index as u32)?;
            face_matches(print, &candidate, diagonal, true).then(|| {
                let center_score = candidate.center.distance(print.center) / (diagonal * 1.0e-2);
                let area_score = (ratio(candidate.area, print.area) - 1.0).abs();
                (index as u32, center_score + area_score)
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.1.total_cmp(&right.1));
    match candidates.as_slice() {
        [(index, _)] => Ok(*index),
        [(index, best), (_, second), ..] if *best < 0.6 * *second => Ok(*index),
        _ => Err(crate::i18n::t("Could not uniquely resolve the referenced face").to_owned()),
    }
}

fn edge_matches(print: &EdgePrint, candidate: &EdgePrint, diagonal: f64, relaxed: bool) -> bool {
    let midpoint_tolerance = diagonal * if relaxed { 1.0e-2 } else { 1.0e-4 };
    let length_ratio = ratio(candidate.length, print.length);
    let (minimum_length, maximum_length) = if relaxed { (0.9, 1.1) } else { (0.99, 1.01) };
    candidate.midpoint.distance(print.midpoint) < midpoint_tolerance
        && (minimum_length..=maximum_length).contains(&length_ratio)
}

fn edge_print(shape: &Shape, index: u32) -> Option<EdgePrint> {
    edge_ref(shape, index).print
}

/// Resolves a persisted edge reference against the current replay shape.
pub(crate) fn resolve_edge(shape: &Shape, reference: &EdgeRef) -> Result<u32, String> {
    let Some(print) = &reference.print else {
        return Ok(reference.index);
    };
    let diagonal = shape_diagonal(shape)?;
    if let Some(candidate) = edge_print(shape, reference.index)
        && (edge_matches(print, &candidate, diagonal, false)
            || edge_matches(print, &candidate, diagonal, true))
    {
        return Ok(reference.index);
    }
    let edge_count = shape.edge_count().map_err(|error| error.to_string())?;
    let mut candidates = (0..edge_count)
        .filter_map(|index| {
            let candidate = edge_print(shape, index as u32)?;
            edge_matches(print, &candidate, diagonal, true).then(|| {
                let center_score =
                    candidate.midpoint.distance(print.midpoint) / (diagonal * 1.0e-2);
                let length_score = (ratio(candidate.length, print.length) - 1.0).abs();
                (index as u32, center_score + length_score, candidate)
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.1.total_cmp(&right.1));
    let mut distinct = Vec::new();
    for candidate in candidates {
        if distinct
            .iter()
            .any(|(_, _, existing): &(u32, f64, EdgePrint)| {
                existing.midpoint.distance(candidate.2.midpoint) < diagonal * 1.0e-8
                    && (ratio(existing.length, candidate.2.length) - 1.0).abs() < 1.0e-8
            })
        {
            continue;
        }
        distinct.push(candidate);
    }
    match distinct.as_slice() {
        [(index, _, _)] => Ok(*index),
        [(index, best, _), (_, second, _), ..] if *best < 0.6 * *second => Ok(*index),
        _ => Err(crate::i18n::t("Could not uniquely resolve the referenced edge").to_owned()),
    }
}

fn body_shape(document: &Document, body: BodyId) -> Result<&Shape, String> {
    document
        .bodies
        .iter()
        .find(|item| item.id == body)
        .map(|item| item.shape.as_ref())
        .ok_or_else(|| format!("missing body {}", body.0))
}

fn resolve_faces(shape: &Shape, references: &[FaceRef]) -> Result<Vec<u32>, String> {
    references
        .iter()
        .map(|reference| resolve_face(shape, reference))
        .collect()
}

fn resolve_edges(shape: &Shape, references: &[EdgeRef]) -> Result<Vec<u32>, String> {
    references
        .iter()
        .map(|reference| resolve_edge(shape, reference))
        .collect()
}

/// Rebuilds a fresh document by invoking the normal mutation entry points.
pub fn replay(steps: &[HistoryStep]) -> Result<Document, ReplayError> {
    let mut document = Document::new_for_replay();
    for (step_index, step) in steps.iter().enumerate() {
        if step.suppressed {
            continue;
        }
        let ok = match &step.op {
            HistoryOp::AddPrimitive { kind } => {
                document.add_primitive(*kind);
                true
            }
            HistoryOp::AddSketch {
                plane,
                support_body,
            } => {
                document.add_sketch_with_support(*plane, *support_body);
                true
            }
            HistoryOp::AddConstructionPlane { plane } => {
                document.add_construction_plane(*plane);
                true
            }
            HistoryOp::AddConstructionAxis { origin, direction } => document
                .add_construction_axis(*origin, *direction)
                .is_some(),
            HistoryOp::AddConstructionPoint { position } => {
                document.add_construction_point(*position).is_some()
            }
            HistoryOp::AddReferenceImage {
                name,
                data,
                width_mm,
                plane,
                origin,
            } => {
                document.add_reference_image_replay(name.clone(), data, *width_mm, *plane, *origin)
            }
            HistoryOp::SketchState {
                sketch,
                entities,
                constraints,
                pinned,
            } => document.apply_sketch_state(
                *sketch,
                entities.clone(),
                constraints.clone(),
                pinned.clone(),
            ),
            HistoryOp::Extrude {
                sketch: Some(sketch),
                profile_index: Some(profile_index),
                body: None,
                face_index: None,
                distance,
                opposite_distance,
                side_mode,
                mode,
            } => {
                let Some(source) = document.sketches.iter().find(|item| item.id == *sketch) else {
                    return Err(failed(step_index, format!("missing sketch {}", sketch.0)));
                };
                let drag = ProfileExtrudeDrag {
                    sketch: *sketch,
                    profile_index: *profile_index,
                    origin: source.plane.origin,
                    normal: source.plane.normal(),
                    distance: distance.value,
                    opposite_distance: opposite_distance.value,
                    side_mode: *side_mode,
                    mode: *mode,
                };
                document.apply_profile_extrude(&drag)
            }
            HistoryOp::Extrude {
                sketch: None,
                profile_index: None,
                body: Some(body),
                face_index: Some(face_index),
                distance,
                opposite_distance,
                side_mode,
                mode,
            } => {
                let index = resolve_face(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    face_index,
                )
                .map_err(|message| failed(step_index, message))?;
                replay_face_extrude(
                    &mut document,
                    *body,
                    index,
                    distance.value,
                    opposite_distance.value,
                    *side_mode,
                    *mode,
                    false,
                )
            }
            HistoryOp::Extrude { .. } => false,
            HistoryOp::SurfaceExtrude {
                sketch,
                entity_indices,
                distance,
                opposite_distance,
                side_mode,
            } => document
                .apply_open_chain_extrude(
                    *sketch,
                    entity_indices,
                    distance.value,
                    opposite_distance.value,
                    *side_mode,
                )
                .is_some(),
            HistoryOp::OffsetFace {
                body,
                face_index,
                distance,
            } => {
                let index = resolve_face(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    face_index,
                )
                .map_err(|message| failed(step_index, message))?;
                replay_face_extrude(
                    &mut document,
                    *body,
                    index,
                    distance.value,
                    0.0,
                    ExtrudeSideMode::OneSided,
                    ExtrudeMode::Auto,
                    true,
                )
            }
            HistoryOp::OffsetFaces { faces, distance } => {
                let mut resolved = Vec::with_capacity(faces.len());
                for (body, reference) in faces {
                    let index = resolve_face(
                        body_shape(&document, *body)
                            .map_err(|message| failed(step_index, message))?,
                        reference,
                    )
                    .map_err(|message| failed(step_index, message))?;
                    resolved.push((*body, index));
                }
                document.apply_offset_faces(&resolved, distance.value)
            }
            HistoryOp::ReplaceFace {
                body,
                face_index,
                target_origin,
                target_normal,
            } => {
                let index = resolve_face(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    face_index,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_replace_face(*body, index, *target_origin, *target_normal)
            }
            HistoryOp::Boolean { op, target, tools } => {
                let ids: Vec<_> = std::iter::once(*target)
                    .chain(tools.iter().copied())
                    .collect();
                document.apply_boolean(*op, &ids)
            }
            HistoryOp::Fillet {
                body,
                edges,
                radius,
                end_radius,
            } => {
                let indices = resolve_edges(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    edges,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_dressup(
                    *body,
                    DressUp::Fillet {
                        radius: radius.value,
                        end_radius: end_radius.as_ref().map(|radius| radius.value),
                        edge_indices: indices,
                    },
                )
            }
            HistoryOp::Chamfer {
                body,
                edges,
                radius,
            } => {
                let indices = resolve_edges(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    edges,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_dressup(
                    *body,
                    DressUp::Chamfer {
                        radius: radius.value,
                        edge_indices: indices,
                    },
                )
            }
            HistoryOp::Shell {
                body,
                faces,
                thickness,
            } => {
                let indices = resolve_faces(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    faces,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_shell(*body, &indices, thickness.value)
            }
            HistoryOp::Hole {
                body,
                face_index,
                at,
                diameter,
                kind,
                cut,
            } => {
                let index = resolve_face(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    face_index,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_hole(*body, index, *at, diameter.value, kind.clone(), cut.clone())
            }
            HistoryOp::Draft {
                body,
                face_indices,
                direction,
                neutral_origin,
                neutral_normal,
                angle_degrees,
            } => {
                let indices = resolve_faces(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    face_indices,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_draft(
                    *body,
                    &indices,
                    *direction,
                    *neutral_origin,
                    *neutral_normal,
                    angle_degrees.value,
                )
            }
            HistoryOp::Transform { ids, op } => {
                document.apply_transform(ids, *op).len() == ids.len()
            }
            HistoryOp::MultiTransform { ids, op, count } => {
                let expected = ids
                    .len()
                    .saturating_mul((*count as usize).saturating_sub(1));
                document
                    .apply_multi_transform(ids, *op, *count as usize)
                    .len()
                    == expected
            }
            HistoryOp::Scale { ids, factor, pivot } => {
                document.apply_scale(ids, factor.value, *pivot).len() == ids.len()
            }
            HistoryOp::Split { body, y } => document.apply_split(*body, *y).len() == 2,
            HistoryOp::Revolve {
                sketch,
                profile_index,
                axis_origin,
                axis_direction,
                angle_degrees,
                mode,
            } => document
                .apply_revolve(
                    crate::document::SelItem::Profile(*sketch, *profile_index),
                    *axis_origin,
                    *axis_direction,
                    angle_degrees.value,
                    *mode,
                )
                .is_some(),
            HistoryOp::SurfaceRevolve {
                sketch,
                entity_indices,
                axis_origin,
                axis_direction,
                angle_degrees,
            } => document
                .apply_open_chain_revolve(
                    *sketch,
                    entity_indices,
                    *axis_origin,
                    *axis_direction,
                    angle_degrees.value,
                )
                .is_some(),
            HistoryOp::Patch { body, edges } => {
                let indices = resolve_edges(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    edges,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_patch(*body, &indices).is_some()
            }
            HistoryOp::Stitch { bodies } => document.apply_stitch(bodies).is_some(),
            HistoryOp::Thicken { body, thickness } => {
                document.apply_thicken(*body, thickness.value)
            }
            HistoryOp::DeleteFace { body, faces } => {
                let indices = resolve_faces(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    faces,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_delete_faces(*body, &indices)
            }
            HistoryOp::Sweep {
                sketch,
                profile_index,
                path,
            } => document
                .apply_sweep((*sketch, *profile_index), path.clone())
                .is_some(),
            HistoryOp::Loft { sections } => document.apply_loft(sections).is_some(),
            HistoryOp::Helix {
                origin,
                axis,
                radius,
                pitch,
                turns,
                profile_radius,
                left_handed,
            } => document
                .apply_helix(
                    *origin,
                    *axis,
                    radius.value,
                    pitch.value,
                    turns.value,
                    profile_radius.value,
                    *left_handed,
                )
                .is_some(),
            HistoryOp::Thread {
                body,
                face_index,
                external,
                mode,
                pitch,
                depth,
            } => {
                let index = resolve_face(
                    body_shape(&document, *body).map_err(|message| failed(step_index, message))?,
                    face_index,
                )
                .map_err(|message| failed(step_index, message))?;
                document.apply_thread(*body, index, *external, *mode, pitch.value, depth.value)
            }
            HistoryOp::Mirror {
                ids,
                plane_origin,
                plane_normal,
            } => {
                document
                    .apply_mirror(ids, *plane_origin, *plane_normal)
                    .len()
                    == ids.len()
            }
            HistoryOp::LinearPattern {
                ids,
                axis_origin,
                axis_direction,
                spacing,
                count,
            } => {
                let expected = ids
                    .len()
                    .saturating_mul((*count as usize).saturating_sub(1));
                document
                    .apply_linear_pattern(
                        ids,
                        *axis_origin,
                        *axis_direction,
                        *count as usize,
                        spacing.value,
                    )
                    .len()
                    == expected
            }
            HistoryOp::CircularPattern {
                ids,
                axis_origin,
                axis_direction,
                count,
            } => {
                let expected = ids
                    .len()
                    .saturating_mul((*count as usize).saturating_sub(1));
                document
                    .apply_circular_pattern(ids, *axis_origin, *axis_direction, *count as usize)
                    .len()
                    == expected
            }
            HistoryOp::ImportFile { path } => document.import_file(path).is_ok(),
            HistoryOp::DeleteBodies { ids } => document.remove_bodies_checked(ids),
            HistoryOp::SetVisible { id, visible } => document.set_visible_checked(*id, *visible),
            HistoryOp::Rename { id, name } => document.rename_checked(*id, name.clone()),
            HistoryOp::SetMaterial { body, material } => {
                document.set_material_checked(*body, *material)
            }
            HistoryOp::AddJoint { joint } => document.add_joint(joint.clone()).is_some(),
            HistoryOp::DeleteJoint { id } => document.delete_joint(*id),
            HistoryOp::SetJointValue { id, value, value2 } => {
                document.set_joint_value(*id, *value, *value2)
            }
            HistoryOp::SetGrounded { body, grounded } => document.set_grounded(*body, *grounded),
        };
        if !ok {
            return Err(failed(step_index, "operation could not be evaluated"));
        }
    }
    document.finish_replay(steps.to_vec());
    Ok(document)
}

fn replay_face_extrude(
    document: &mut Document,
    body: BodyId,
    face_index: u32,
    distance: f64,
    opposite_distance: f64,
    side_mode: ExtrudeSideMode,
    mode: ExtrudeMode,
    offset: bool,
) -> bool {
    let Some(source) = document.bodies.iter().find(|item| item.id == body) else {
        return false;
    };
    let Some((origin, normal)) = face_frame(&source.shape, face_index) else {
        return false;
    };
    let drag = ExtrudeDrag {
        body,
        face_index,
        origin,
        normal,
        distance,
        opposite_distance,
        side_mode,
        mode,
    };
    if offset {
        document.apply_offset_face(&drag)
    } else {
        document.apply_extrude(&drag)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn num_deserializes_legacy_number_and_roundtrips_expression() {
        let literal: Num = serde_json::from_str("12.5").unwrap();
        assert_eq!(literal, Num::literal(12.5));
        assert_eq!(serde_json::to_string(&literal).unwrap(), "12.5");

        let expression = Num::from_input(25.0, "width * 2".to_owned());
        let json = serde_json::to_string(&expression).unwrap();
        assert_eq!(serde_json::from_str::<Num>(&json).unwrap(), expression);
        assert!(json.contains("width * 2"));
    }
    use crate::sketch::SketchEntity;

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

    #[test]
    fn add_construction_plane_replays() {
        let mut document = Document::new();
        let plane = SketchPlane {
            origin: DVec3::new(0.0, 0.0, 12.0),
            ..SketchPlane::xy()
        };
        document.add_construction_plane(plane);
        let replayed = replay(&document.history).expect("replay construction plane");
        assert_eq!(replayed.construction_planes.len(), 1);
        assert_eq!(replayed.construction_planes[0].plane, plane);
    }

    #[test]
    fn sketch_on_construction_plane_roundtrips() {
        let mut document = Document::new();
        let plane = SketchPlane {
            origin: DVec3::new(3.0, 4.0, 5.0),
            x_axis: DVec3::Y,
            y_axis: DVec3::Z,
        };
        let datum = document.add_construction_plane(plane);
        let sketch = document.add_sketch(
            document
                .construction_planes
                .iter()
                .find(|item| item.id == datum)
                .expect("datum")
                .plane,
        );
        document.finish_sketch_mode();
        let replayed = replay(&document.history).expect("replay datum sketch");
        assert_eq!(replayed.sketches[0].id, sketch);
        assert_eq!(replayed.sketches[0].plane, plane);
    }

    fn box_kind(min: DVec3, max: DVec3) -> PrimitiveKind {
        PrimitiveKind::Box { min, max }
    }

    fn rectangle(document: &mut Document, plane: SketchPlane, size: glam::DVec2) -> SketchId {
        let sketch = document.add_sketch(plane);
        let points = [
            glam::DVec2::ZERO,
            glam::DVec2::new(size.x, 0.0),
            size,
            glam::DVec2::new(0.0, size.y),
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

    fn assert_replay_matches(original: &mut Document) {
        let history = original.replayable_history();
        let replayed = replay(&history).expect("replay modeling operation");
        let expected_ids: Vec<_> = original.bodies.iter().map(|body| body.id).collect();
        let actual_ids: Vec<_> = replayed.bodies.iter().map(|body| body.id).collect();
        assert_eq!(actual_ids, expected_ids);
        for (actual, expected) in replayed.bodies.iter().zip(&original.bodies) {
            let actual = aabb(&actual.shape);
            let expected = aabb(&expected.shape);
            assert!((actual.min() - expected.min()).length() < 1.0e-5);
            assert!((actual.max() - expected.max()).length() < 1.0e-5);
        }
    }

    fn top_face(document: &Document, body: BodyId) -> (u32, DVec3, DVec3) {
        let shape = &document
            .bodies
            .iter()
            .find(|candidate| candidate.id == body)
            .expect("body")
            .shape;
        (0..shape.face_count().unwrap())
            .filter_map(|index| {
                let (origin, normal) = face_frame(shape, index as u32)?;
                Some((
                    index as u32,
                    origin,
                    normal,
                    shape.face_center_of_mass(index).ok()?.z,
                ))
            })
            .max_by(|left, right| left.3.total_cmp(&right.3))
            .map(|(index, origin, normal, _)| (index, origin, normal))
            .expect("top face")
    }

    fn scripted_document() -> Document {
        let mut document = Document::new();
        let body = document.add_primitive(box_kind(DVec3::ZERO, DVec3::splat(10.0)));
        let (face_index, origin, normal) = top_face(&document, body);
        assert!(document.apply_extrude(&ExtrudeDrag {
            body,
            face_index,
            origin,
            normal,
            distance: 5.0,
            opposite_distance: 0.0,
            side_mode: ExtrudeSideMode::OneSided,
            mode: ExtrudeMode::Auto,
        }));
        assert!(document.apply_dressup(
            body,
            DressUp::Fillet {
                radius: 0.5,
                end_radius: None,
                edge_indices: vec![0],
            },
        ));
        assert_eq!(
            document
                .apply_mirror(&[body], DVec3::new(0.0, 3.0, 0.0), DVec3::Y)
                .len(),
            1
        );
        document
    }

    #[test]
    fn record_replay_matches_scripted_body_count_and_bounds() {
        let original = scripted_document();
        let replayed = replay(&original.history).expect("replay scripted history");
        assert_eq!(replayed.bodies.len(), original.bodies.len());
        for (actual, expected) in replayed.bodies.iter().zip(&original.bodies) {
            let actual = aabb(&actual.shape);
            let expected = aabb(&expected.shape);
            assert!((actual.min() - expected.min()).length() < 1.0e-5);
            assert!((actual.max() - expected.max()).length() < 1.0e-5);
        }
    }

    #[test]
    fn picked_axis_and_plane_parameters_survive_replay() {
        let mut revolve = Document::new();
        let sketch = rectangle(
            &mut revolve,
            SketchPlane {
                origin: DVec3::new(0.0, 10.0, 0.0),
                x_axis: DVec3::X,
                y_axis: DVec3::Y,
            },
            glam::DVec2::new(5.0, 10.0),
        );
        assert!(
            revolve
                .apply_revolve(
                    crate::document::SelItem::Profile(sketch, 0),
                    DVec3::ZERO,
                    DVec3::X,
                    270.0,
                    ExtrudeMode::NewBody,
                )
                .is_some()
        );
        assert_replay_matches(&mut revolve);

        let mut copies = Document::new();
        let body = copies.add_primitive(box_kind(
            DVec3::new(2.0, 0.0, 0.0),
            DVec3::new(4.0, 2.0, 2.0),
        ));
        assert_eq!(
            copies
                .apply_linear_pattern(&[body], DVec3::ZERO, DVec3::Y, 2, 7.0)
                .len(),
            1
        );
        assert_eq!(
            copies
                .apply_circular_pattern(&[body], DVec3::new(1.0, 0.0, 0.0), DVec3::X, 3)
                .len(),
            2
        );
        assert_eq!(
            copies
                .apply_mirror(&[body], DVec3::new(5.0, 0.0, 0.0), DVec3::X)
                .len(),
            1
        );
        assert_replay_matches(&mut copies);
    }

    #[test]
    fn editing_extrude_distance_changes_replayed_bounds() {
        let mut document = Document::new();
        let body = document.add_primitive(box_kind(DVec3::ZERO, DVec3::splat(10.0)));
        let (face_index, origin, normal) = top_face(&document, body);
        assert!(document.apply_extrude(&ExtrudeDrag {
            body,
            face_index,
            origin,
            normal,
            distance: 5.0,
            opposite_distance: 0.0,
            side_mode: ExtrudeSideMode::OneSided,
            mode: ExtrudeMode::Auto,
        }));
        assert!(document.history[1].op.set_numeric_value(12.0));
        let replayed = replay(&document.history).expect("replay edited extrusion");
        let bounds = aabb(&replayed.bodies[0].shape);
        assert!((bounds.max().z - 22.0).abs() < 1.0e-5);
    }

    #[test]
    fn symmetric_extrude_side_parameters_survive_replay() {
        let mut document = Document::new();
        let body = document.add_primitive(box_kind(DVec3::ZERO, DVec3::splat(10.0)));
        let source = document.bodies.iter().find(|item| item.id == body).unwrap();
        let (face_index, origin, normal) = (0..source.shape.face_count().unwrap())
            .find_map(|index| {
                let center = source.shape.face_center_of_mass(index).ok()?;
                ((center.z - 10.0).abs() < 1.0e-6)
                    .then(|| {
                        face_frame(&source.shape, index as u32)
                            .map(|(_, n)| (index as u32, center, n))
                    })
                    .flatten()
            })
            .expect("top face");
        assert!(document.apply_extrude(&ExtrudeDrag {
            body,
            face_index,
            origin,
            normal,
            distance: 6.0,
            opposite_distance: 0.0,
            side_mode: ExtrudeSideMode::Symmetric,
            mode: ExtrudeMode::NewBody,
        }));
        assert_replay_matches(&mut document);
    }

    #[test]
    fn deleting_dependency_reports_boolean_index_and_keeps_document() {
        let mut document = Document::new();
        let target = document.add_primitive(box_kind(DVec3::ZERO, DVec3::splat(10.0)));
        let tool = document.add_primitive(box_kind(DVec3::splat(5.0), DVec3::splat(15.0)));
        assert!(document.apply_boolean(BooleanOp::Union, &[target, tool]));
        let before = aabb(&document.bodies[0].shape);
        let mut edited = document.history.clone();
        edited.remove(1);
        let error = document
            .replace_history(edited)
            .expect_err("missing boolean tool must fail");
        assert_eq!(error.step_index, 1);
        assert_eq!(document.bodies.len(), 1);
        let after = aabb(&document.bodies[0].shape);
        assert!((after.min() - before.min()).length() < 1.0e-6);
        assert!((after.max() - before.max()).length() < 1.0e-6);
    }

    #[test]
    fn suppressed_step_is_skipped() {
        let mut document = Document::new();
        document.add_primitive(box_kind(DVec3::ZERO, DVec3::ONE));
        document.history[0].suppressed = true;
        let replayed = replay(&document.history).expect("suppressed replay");
        assert!(replayed.bodies.is_empty());
    }

    #[test]
    fn replay_preserves_body_id_sequence() {
        let original = scripted_document();
        let expected: Vec<_> = original.bodies.iter().map(|body| body.id).collect();
        let replayed = replay(&original.history).expect("deterministic replay");
        let actual: Vec<_> = replayed.bodies.iter().map(|body| body.id).collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn sweep_and_loft_replay_with_deterministic_ids_and_bounds() {
        let mut sweep = Document::new();
        let section = sweep.add_sketch(SketchPlane {
            origin: DVec3::ZERO,
            x_axis: DVec3::Y,
            y_axis: DVec3::Z,
        });
        assert!(sweep.add_sketch_entities(
            section,
            [SketchEntity::Circle {
                center: glam::DVec2::ZERO,
                radius: 1.5,
            }]
        ));
        let path = sweep.add_sketch(SketchPlane::xy());
        assert!(sweep.add_sketch_entities(
            path,
            [
                SketchEntity::Line {
                    a: glam::DVec2::ZERO,
                    b: glam::DVec2::new(15.0, 0.0),
                },
                SketchEntity::Line {
                    a: glam::DVec2::new(15.0, 0.0),
                    b: glam::DVec2::new(15.0, 20.0),
                },
            ]
        ));
        assert!(
            sweep
                .apply_sweep(
                    (section, 0),
                    PathRef::OpenChain {
                        sketch: path,
                        entity_indices: vec![0, 1],
                    },
                )
                .is_some()
        );
        assert_replay_matches(&mut sweep);

        let mut loft = Document::new();
        let lower = rectangle(&mut loft, SketchPlane::xy(), glam::DVec2::new(16.0, 12.0));
        let upper = rectangle(
            &mut loft,
            SketchPlane {
                origin: DVec3::new(1.0, 2.0, 50.0),
                ..SketchPlane::xy()
            },
            glam::DVec2::new(8.0, 6.0),
        );
        assert!(loft.apply_loft(&[(lower, 0), (upper, 0)]).is_some());
        assert_replay_matches(&mut loft);
    }

    #[test]
    fn undo_restores_geometry_and_history_before_parameter_edit() {
        let mut document = Document::new();
        let body = document.add_primitive(box_kind(DVec3::ZERO, DVec3::splat(10.0)));
        let (face_index, origin, normal) = top_face(&document, body);
        assert!(document.apply_extrude(&ExtrudeDrag {
            body,
            face_index,
            origin,
            normal,
            distance: 5.0,
            opposite_distance: 0.0,
            side_mode: ExtrudeSideMode::OneSided,
            mode: ExtrudeMode::Auto,
        }));
        let mut edited = document.history.clone();
        assert!(edited[1].op.set_numeric_value(12.0));
        document.replace_history(edited).expect("edit history");
        assert!((aabb(&document.bodies[0].shape).max().z - 22.0).abs() < 1.0e-5);
        assert!(document.undo());
        assert!((aabb(&document.bodies[0].shape).max().z - 15.0).abs() < 1.0e-5);
        assert_eq!(document.history[1].op.numeric_value(), Some(5.0));
        assert!(document.redo());
        assert_eq!(document.history[1].op.numeric_value(), Some(12.0));
    }

    #[test]
    fn sketch_edits_flush_as_one_coarse_state_on_exit() {
        let mut document = Document::new();
        let sketch = document.add_sketch(SketchPlane::xy());
        assert!(document.add_sketch_entities(
            sketch,
            [SketchEntity::Circle {
                center: glam::DVec2::new(3.0, 4.0),
                radius: 2.0,
            }],
        ));
        assert_eq!(document.history.len(), 1);
        document.finish_sketch_mode();
        assert_eq!(document.history.len(), 2);
        assert!(matches!(
            document.history[1].op,
            HistoryOp::SketchState { sketch: id, .. } if id == sketch
        ));
        let replayed = replay(&document.history).expect("replay sketch state");
        assert_eq!(replayed.sketches[0].entities, document.sketches[0].entities);
    }

    #[test]
    fn legacy_constant_fillet_deserializes_without_end_radius() {
        let json = r#"{"t":"Fillet","v":{"body":1,"edges":[0],"radius":2.0}}"#;
        let op: HistoryOp = serde_json::from_str(json).expect("legacy fillet");
        let HistoryOp::Fillet {
            edges,
            end_radius: None,
            ..
        } = op
        else {
            panic!("legacy fillet representation");
        };
        assert_eq!(edges, vec![EdgeRef::from(0)]);
        assert_eq!(serde_json::to_string(&EdgeRef::from(7)).unwrap(), "7");
        assert_eq!(serde_json::to_string(&FaceRef::from(3)).unwrap(), "3");
        let steps = vec![
            HistoryStep::new(HistoryOp::AddPrimitive {
                kind: PrimitiveKind::Box {
                    min: DVec3::ZERO,
                    max: DVec3::splat(10.0),
                },
            }),
            HistoryStep::new(HistoryOp::Fillet {
                body: BodyId(1),
                edges,
                radius: 1.0.into(),
                end_radius: None,
            }),
        ];
        assert!(replay(&steps).is_ok(), "plain indices retain legacy replay");
    }

    #[test]
    fn face_fingerprint_fallback_ignores_a_stale_index() {
        let shape = PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::new(10.0, 20.0, 30.0),
        }
        .shape();
        let target = 0;
        let mut reference = face_ref(&shape, target);
        reference.index = (1..shape.face_count().unwrap() as u32)
            .find(|&index| {
                let candidate = face_ref(&shape, index);
                candidate.print != reference.print
            })
            .expect("different face index");
        assert_eq!(resolve_face(&shape, &reference).unwrap(), target);
    }

    #[test]
    fn identical_face_candidates_are_reported_as_ambiguous() {
        let first = PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::new(10.0, 20.0, 30.0),
        }
        .shape();
        let second = first.try_clone().unwrap();
        let mut reference = face_ref(&first, 0);
        reference.index = u32::MAX;
        let compound = Shape::compound(vec![first, second]).unwrap();
        assert_eq!(
            resolve_face(&compound, &reference).unwrap_err(),
            crate::i18n::t("Could not uniquely resolve the referenced face")
        );
    }

    #[test]
    fn suppressed_upstream_hole_keeps_downstream_fillet_on_the_same_edge() {
        let mut document = Document::new();
        let body = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(20.0),
        });
        let (top, _, _) = top_face(&document, body);
        assert!(document.apply_hole(
            body,
            top,
            DVec3::new(1.0, 1.0, 20.0),
            4.0,
            HoleKind::Through,
            HoleCut::None,
        ));
        let holed = document.bodies[0].shape.as_ref();
        let plain = PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::splat(20.0),
        }
        .shape();
        let (edge_index, edge_reference, plain_index) = (0..holed.edge_count().unwrap() as u32)
            .find_map(|index| {
                let reference = edge_ref(holed, index);
                let resolved = resolve_edge(&plain, &reference).ok()?;
                (resolved != index).then_some((index, reference, resolved))
            })
            .expect("hole changes at least one preserved box edge index");
        assert!(document.apply_dressup(
            body,
            DressUp::Fillet {
                radius: 1.0,
                end_radius: None,
                edge_indices: vec![edge_index],
            },
        ));
        let expected = plain.fillet_edges(1.0, &[plain_index]).unwrap();
        let expected_faces = expected.face_count().unwrap();
        let mut edited = document.history.clone();
        edited[1].suppressed = true;
        let HistoryOp::Fillet { edges, .. } = &edited[2].op else {
            panic!("fillet history step");
        };
        assert_eq!(edges[0], edge_reference);
        assert_eq!(resolve_edge(&plain, &edges[0]).unwrap(), plain_index);
        let replayed = replay(&edited).expect("fingerprint fallback replay");
        assert_eq!(
            replayed.bodies[0].shape.face_count().unwrap(),
            expected_faces
        );
        let actual_mass = replayed.bodies[0].shape.volume_properties().unwrap();
        let expected_mass = expected.volume_properties().unwrap();
        assert!((actual_mass.volume - expected_mass.volume).abs() < 1.0e-6);
        assert!(
            (actual_mass.center_of_mass - expected_mass.center_of_mass).length() < 1.0e-6,
            "filleted material location must remain stable"
        );
    }
}
