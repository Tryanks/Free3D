//! gpui viewport view, input routing, picking, and RenderImage lifecycle.

mod grid;
mod orientation_cube;
mod reference_image;
mod renderer;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use glam::{DVec3, Mat4, Vec2, Vec3};
use gpui::{
    Context, Entity, EventEmitter, FocusHandle, ImageSource, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PinchEvent, Render, RenderImage, ScrollDelta,
    ScrollWheelEvent, Subscription, Window, div, img, prelude::*, px, rgba,
};
use image::{Frame, RgbaImage};
use smallvec::smallvec;

use crate::{
    assembly::{Connector, ConnectorFrame, ConnectorSource, Joint, JointId, JointKind},
    camera::{OrbitCamera, frame_face_target},
    commands::{ModeChip, StandardView, ToolId},
    constraint::{Constraint, EntityRef, PointRef},
    document::{
        BodyId, BodyKind, Document, DressUp, Material, SelItem, SelectionFilter, TransformOp,
    },
    gizmo::{Handle, axis_drag_parameter, hit_test, hit_test_axis, ray_plane, snap_angle},
    history::{HoleCut, HoleKind, edge_ref, face_ref},
    kernel::{BodyMesh, tessellate},
    nav::{GestureKind, NavAction, NavPreset, resolve, scroll_gesture},
    pick::{FaceHit, PickBody, fit_straight_edge, pick_all, pick_edge, pick_face},
    saved_views::SavedView,
    sketch::{
        Profile, Sketch, SketchEntity, SketchId, SketchItem, SketchPlane, arc_center_radius,
        centered_rectangle, ellipse_point, regular_polygon, rounded_rectangle, sample_arc,
        sample_ellipse, sample_ellipse_arc, sample_spline, slot, snap_horizontal_vertical,
        tangent_arc_mid, three_point_circle, three_tangent_circle, two_tangent_circle,
    },
    theme::Theme,
    tools::extrude::{
        ExtrudeDrag, ExtrudeMode, ExtrudeSideMode, OpenChainExtrudeDrag, ProfileExtrudeDrag,
        cursor_distance, face_frame, open_chain_prism, prism as extrude_prism, profile_prism,
    },
    tools::{
        dressup::{DressUpDrag, edge_frame, preview as dressup_preview},
        shell::{ShellDrag, preview as shell_preview},
    },
    ui::{
        expr,
        numeric_input::{NumericInput, NumericInputEvent},
    },
    units::Units,
};
use orientation_cube::Region as CubeRegion;
use renderer::{ExtrudeArrowRender, GizmoRender, OrientationCubeRender, Renderer};

/// Body/edge presentation used by the right-side display row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DisplayMode {
    /// Lit faces and visible feature edges.
    #[default]
    Shaded,
    /// Feature edges only.
    Wireframe,
    /// Shaded faces plus faint depth-occluded feature edges.
    HiddenEdges,
    /// Fixed-alpha shaded bodies with fully opaque feature edges.
    XRay,
}

impl DisplayMode {
    /// Chinese state label shown below the display-mode row.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Shaded => "着色",
            Self::Wireframe => "线框",
            Self::HiddenEdges => "隐藏边",
            Self::XRay => "X-Ray",
        }
    }

    /// Advances to the next display mode.
    pub const fn next(self) -> Self {
        match self {
            Self::Shaded => Self::Wireframe,
            Self::Wireframe => Self::HiddenEdges,
            Self::HiddenEdges => Self::XRay,
            Self::XRay => Self::Shaded,
        }
    }
}

/// Mutually exclusive surface-analysis overlay.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AnalysisMode {
    /// Normal shaded material.
    #[default]
    Off,
    /// Reflection-vector inspection stripes.
    Zebra,
    /// Approximate normal-variation curvature ramp.
    Curvature,
}

/// Events the viewport sends to keep parent chrome state synchronized.
pub enum ViewportEvent {
    /// Escape exited every active bottom-left viewport mode.
    ModesExited,
}

impl EventEmitter<ViewportEvent> for Viewport {}

const SECTION_NORMAL: Vec3 = Vec3::Y;

fn point_is_clipped(point: Vec3, normal: Vec3, offset: f32) -> bool {
    point.dot(normal) > offset
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Measurement {
    first: Vec3,
    second: Vec3,
}

impl Measurement {
    fn delta(self) -> Vec3 {
        self.second - self.first
    }

    fn distance(self) -> f32 {
        self.delta().length()
    }
}

struct SectionDrag {
    origin: Vec3,
    start_offset: f32,
    start_parameter: f32,
}

fn save_dump_frame(rendered: &renderer::RenderedFrame) {
    let Ok(path) = std::env::var("FREE3D_DUMP_FRAME") else {
        return;
    };
    let mut rgba = rendered.bgra.clone();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    if let Err(error) = image::save_buffer(
        &path,
        &rgba,
        rendered.width,
        rendered.height,
        image::ColorType::Rgba8,
    ) {
        eprintln!("FREE3D_DUMP_FRAME failed: {error}");
    }
}

fn polygon_contains(vertices: &[glam::DVec2], point: glam::DVec2) -> bool {
    let mut inside = false;
    for (a, b) in vertices
        .iter()
        .zip(vertices.iter().cycle().skip(1))
        .take(vertices.len())
    {
        if (a.y > point.y) != (b.y > point.y)
            && point.x < (b.x - a.x) * (point.y - a.y) / (b.y - a.y) + a.x
        {
            inside = !inside;
        }
    }
    inside
}

fn profile_contains(sketch: &Sketch, profile: &Profile, point: glam::DVec2) -> bool {
    match profile {
        Profile::Circle { center, radius } => center.distance_squared(point) <= radius * radius,
        Profile::Ellipse {
            center,
            major,
            minor_ratio,
        } => polygon_contains(&sample_ellipse(*center, *major, *minor_ratio, 64), point),
        Profile::LineLoop(vertices) => polygon_contains(vertices, point),
        Profile::CurveLoop(curves) => {
            let vertices: Vec<_> = curves
                .iter()
                .flat_map(|(index, reversed)| {
                    let mut points = match &sketch.entities[*index].geo {
                        SketchEntity::Line { a, b } => vec![*a, *b],
                        SketchEntity::Arc { start, end, mid } => sample_arc(*start, *mid, *end, 32),
                        SketchEntity::Spline { points } => sketch.spline_polyline(*index, points),
                        SketchEntity::CvSpline { control, degree } => {
                            crate::sketch::sample_cv_spline(control, *degree, sketch.plane)
                        }
                        SketchEntity::EllipseArc {
                            center,
                            major,
                            minor_ratio,
                            start_angle,
                            end_angle,
                        } => sample_ellipse_arc(
                            *center,
                            *major,
                            *minor_ratio,
                            *start_angle,
                            *end_angle,
                            32,
                        ),
                        SketchEntity::Point { .. } => Vec::new(),
                        SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => Vec::new(),
                    };
                    if *reversed {
                        points.reverse();
                    }
                    points
                })
                .collect();
            polygon_contains(&vertices, point)
        }
    }
}

fn point_segment_distance(point: Vec2, a: Vec2, b: Vec2) -> f32 {
    let segment = b - a;
    if segment.length_squared() <= f32::EPSILON {
        return point.distance(a);
    }
    let t = ((point - a).dot(segment) / segment.length_squared()).clamp(0.0, 1.0);
    point.distance(a + segment * t)
}

fn primitive_segments(entity: SketchEntity) -> Vec<(glam::DVec2, glam::DVec2)> {
    match entity {
        SketchEntity::Line { a, b } => vec![(a, b)],
        SketchEntity::Arc { start, mid, end } => sample_arc(start, mid, end, 32)
            .windows(2)
            .map(|pair| (pair[0], pair[1]))
            .collect(),
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy)]
enum DragAnchor {
    Axis(f32),
    Ring(Vec3),
    Center(Vec3),
}

struct GizmoDrag {
    handle: Handle,
    pivot: Vec3,
    ids: Vec<BodyId>,
    anchor: DragAnchor,
    current: Option<TransformOp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HelixPhase {
    Radius,
    Pitch,
    Turns,
    ProfileRadius,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ThreadPhase {
    Pitch,
    Depth,
}

struct ThreadInteraction {
    body: BodyId,
    face: u32,
    external: bool,
    mode: crate::document::ThreadMode,
    pitch: f64,
    depth: f64,
    phase: ThreadPhase,
    origin: DVec3,
    axis: DVec3,
    radius: f64,
}

struct HelixInteraction {
    axis: AxisReference,
    radius: f64,
    pitch: f64,
    turns: f64,
    profile_radius: f64,
    left_handed: bool,
    phase: HelixPhase,
    expressions: [Option<String>; 4],
}

struct ExtrudeInteraction {
    drag: ExtrudeDrag,
    anchor: f64,
    bbox_diagonal: f64,
    last_preview_distance: Option<f64>,
    opposite_phase: bool,
    expressions: [Option<String>; 2],
}

struct ProfileExtrudeInteraction {
    drag: ProfileExtrudeDrag,
    anchor: f64,
    bbox_diagonal: f64,
    last_preview_distance: Option<f64>,
    opposite_phase: bool,
    expressions: [Option<String>; 2],
}

struct OpenChainExtrudeInteraction {
    drag: OpenChainExtrudeDrag,
    anchor: f64,
    bbox_diagonal: f64,
    last_preview_distance: Option<f64>,
    opposite_phase: bool,
    expressions: [Option<String>; 2],
}

struct DressUpInteraction {
    drag: DressUpDrag,
    anchor: f64,
    bbox_diagonal: f64,
    last_preview_radius: Option<f64>,
    expression: Option<String>,
    variable_start_entered: bool,
}

struct ShellInteraction {
    drag: ShellDrag,
    anchor: f64,
    bbox_diagonal: f64,
    last_preview_thickness: Option<f64>,
    expression: Option<String>,
}

struct ThickenInteraction {
    body: BodyId,
    origin: DVec3,
    direction: DVec3,
    thickness: f64,
    anchor: f64,
    bbox_diagonal: f64,
    last_preview_thickness: Option<f64>,
    expression: Option<String>,
}

struct RevolveInteraction {
    source: RevolveSource,
    axis: AxisReference,
    angle_degrees: f64,
    start_x: Option<f32>,
    moved: bool,
    mode: ExtrudeMode,
    expression: Option<String>,
}

#[derive(Clone, Debug)]
enum RevolveSource {
    Profile(SelItem),
    OpenChain {
        sketch: SketchId,
        entity_indices: Vec<usize>,
    },
}

struct HoleInteraction {
    body: BodyId,
    face_index: u32,
    plane: SketchPlane,
    at: Option<DVec3>,
    diameter: f64,
    kind: HoleKind,
    cut: HoleCut,
    start_x: Option<f32>,
    phase: HolePhase,
    diameter_expression: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HolePhase {
    Location,
    Diameter,
    Depth,
    CounterboreDiameter,
    CounterboreDepth,
    CountersinkDiameter,
}

struct DraftInteraction {
    body: BodyId,
    faces: Vec<u32>,
    neutral: PlaneReference,
    angle_degrees: f64,
    start_x: Option<f32>,
    expression: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PatternMode {
    Linear,
    Circular,
}

impl PatternMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Linear => "线性",
            Self::Circular => "环形",
        }
    }
}

struct PatternInteraction {
    ids: Vec<BodyId>,
    mode: PatternMode,
    count: usize,
    spacing: f64,
    axis: Option<AxisReference>,
    start_x: Option<f32>,
    moved: bool,
    expression: Option<String>,
}

struct SketchPatternInteraction {
    id: SketchId,
    entities: Vec<usize>,
    mode: PatternMode,
    count: usize,
    spacing: f64,
    anchor: Option<glam::DVec2>,
    direction: glam::DVec2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReferenceKind {
    Axis,
    Plane,
}

#[derive(Clone, Copy, Debug)]
struct AxisReference {
    origin: DVec3,
    direction: DVec3,
    label: &'static str,
}

#[derive(Clone, Copy, Debug)]
struct PlaneReference {
    origin: DVec3,
    normal: DVec3,
    label: &'static str,
}

#[derive(Clone, Copy, Debug)]
enum ReferenceGeometry {
    Axis(AxisReference),
    Plane(PlaneReference),
}

enum PendingReference {
    Revolve {
        source: RevolveSource,
        default: AxisReference,
    },
    Mirror {
        ids: Vec<BodyId>,
        default: PlaneReference,
    },
    Draft {
        body: BodyId,
        faces: Vec<u32>,
        default: PlaneReference,
    },
    Pattern,
    ReplaceFace {
        body: BodyId,
        face_index: u32,
    },
    Helix,
    SketchMirror {
        id: SketchId,
        entities: Vec<usize>,
    },
}

fn world_reference(key: &str, kind: ReferenceKind) -> Option<ReferenceGeometry> {
    let (direction, axis_label, plane_label) = match key.to_ascii_lowercase().as_str() {
        "x" => (DVec3::X, "world X", "world YZ"),
        "y" => (DVec3::Y, "world Y", "world ZX"),
        "z" => (DVec3::Z, "world Z", "world XY"),
        _ => return None,
    };
    Some(match kind {
        ReferenceKind::Axis => ReferenceGeometry::Axis(AxisReference {
            origin: DVec3::ZERO,
            direction,
            label: axis_label,
        }),
        ReferenceKind::Plane => ReferenceGeometry::Plane(PlaneReference {
            origin: DVec3::ZERO,
            normal: direction,
            label: plane_label,
        }),
    })
}

enum M6Interaction {
    ConstructionPlane {
        base: SketchPlane,
        distance: f64,
        drag: Option<(f32, f64)>,
    },
    Scale {
        ids: Vec<BodyId>,
        pivot: DVec3,
        factor: f64,
        drag: Option<(f32, f64)>,
    },
    Split {
        id: BodyId,
        y: f64,
        drag: Option<(f32, f64)>,
    },
    Align {
        ids: Vec<BodyId>,
        axes: [bool; 3],
    },
}

struct CubeInteraction {
    press_pointer: Vec2,
    pressed_region: Option<CubeRegion>,
    dragged: bool,
}

struct SketchInteraction {
    id: SketchId,
    plane: SketchPlane,
    tool: ToolId,
    anchor: Option<glam::DVec2>,
    anchor_ref: Option<PointRef>,
    arc_end: Option<glam::DVec2>,
    arc_end_ref: Option<PointRef>,
    chain_start: Option<glam::DVec2>,
    cursor: Option<glam::DVec2>,
    hv_snapped: bool,
    fillet_first: Option<usize>,
    fillet_pair: Option<(usize, usize)>,
    edit_profile: Option<usize>,
    edit_distance: f64,
    trim_preview: Option<SketchEntity>,
    spline_points: Vec<glam::DVec2>,
    spline_start_ref: Option<PointRef>,
    spline_end_ref: Option<PointRef>,
    tangent_arc: bool,
    chain_tangent: Option<glam::DVec2>,
    aux_point: Option<glam::DVec2>,
    phase_value: Option<f64>,
    polygon_sides: usize,
}

struct SketchEntityDrag {
    id: SketchId,
    entity: usize,
    start: Sketch,
    pointer_start: glam::DVec2,
    moved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum SketchDimensionTarget {
    Length(EntityRef),
    Radius(EntityRef),
    Diameter(EntityRef),
    Distance { a: PointRef, b: PointRef },
    HDistance { a: PointRef, b: PointRef, sign: f64 },
    VDistance { a: PointRef, b: PointRef, sign: f64 },
    Angle { a: EntityRef, b: EntityRef },
}

#[derive(Clone)]
struct DimensionReadout {
    id: SketchId,
    target: SketchDimensionTarget,
    label: String,
    position: Vec2,
    value: f64,
    reference: bool,
    expression: Option<String>,
}

struct SceneMesh {
    id: BodyId,
    visible: bool,
    shape: Arc<occt::Shape>,
    mesh: BodyMesh,
    material: Material,
    kind: BodyKind,
    pose: Mat4,
}

fn scene_upload_list<'a>(
    scene: &'a [SceneMesh],
    isolated: Option<&HashSet<BodyId>>,
) -> Vec<(BodyId, &'a BodyMesh, bool, Material)> {
    scene
        .iter()
        .filter(|body| body.visible && isolated.is_none_or(|isolated| isolated.contains(&body.id)))
        .map(|body| {
            (
                body.id,
                &body.mesh,
                body.kind == BodyKind::Surface,
                body.material,
            )
        })
        .collect()
}

fn body_render_mesh(body: &crate::document::Body) -> BodyMesh {
    let mut mesh = tessellate(&body.shape, 0.25);
    for thread in &body.cosmetic_threads {
        let axis = thread.axis.normalize_or_zero();
        let reference = if axis.x.abs() < 0.8 {
            DVec3::X
        } else {
            DVec3::Y
        };
        let x = axis.cross(reference).normalize_or_zero();
        let y = axis.cross(x).normalize_or_zero();
        let turns = thread.depth / thread.pitch;
        let segments = (turns * 48.0).ceil().max(8.0) as usize;
        let points: Vec<_> = (0..=segments)
            .map(|index| {
                let t = index as f64 / segments as f64;
                let angle = std::f64::consts::TAU * turns * t;
                let point = thread.origin
                    + axis * (thread.depth * t)
                    + (x * angle.cos() + y * angle.sin()) * thread.radius;
                [point.x as f32, point.y as f32, point.z as f32]
            })
            .collect();
        for segment in points.windows(2) {
            mesh.edge_vertices.extend_from_slice(segment);
        }
    }
    mesh
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumericDragTransition {
    Ignore,
    Freeze(char),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenRect {
    minimum: Vec2,
    maximum: Vec2,
}

impl ScreenRect {
    fn from_points(a: Vec2, b: Vec2) -> Self {
        Self {
            minimum: a.min(b),
            maximum: a.max(b),
        }
    }

    fn from_projected_points(points: impl IntoIterator<Item = Vec2>) -> Option<Self> {
        let mut points = points.into_iter().filter(|point| point.is_finite());
        let first = points.next()?;
        let (minimum, maximum) = points.fold((first, first), |(minimum, maximum), point| {
            (minimum.min(point), maximum.max(point))
        });
        Some(Self { minimum, maximum })
    }

    fn contains_rect(self, other: Self) -> bool {
        other.minimum.cmpge(self.minimum).all() && other.maximum.cmple(self.maximum).all()
    }

    fn intersects(self, other: Self) -> bool {
        self.minimum.cmple(other.maximum).all() && self.maximum.cmpge(other.minimum).all()
    }

    fn contains_point(self, point: Vec2) -> bool {
        point.cmpge(self.minimum).all() && point.cmple(self.maximum).all()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MarqueeMode {
    Window,
    Crossing,
}

fn marquee_mode(start: Vec2, current: Vec2) -> MarqueeMode {
    if current.x >= start.x {
        MarqueeMode::Window
    } else {
        MarqueeMode::Crossing
    }
}

fn screen_bounds_match(selection: ScreenRect, bounds: ScreenRect, mode: MarqueeMode) -> bool {
    match mode {
        MarqueeMode::Window => selection.contains_rect(bounds),
        MarqueeMode::Crossing => selection.intersects(bounds),
    }
}

struct MarqueeInteraction {
    start: Vec2,
    current: Vec2,
    shift: bool,
    active: bool,
}

#[derive(Clone, Copy)]
struct JointDriveInteraction {
    id: JointId,
    kind: JointKind,
    start_x: f32,
    start_value: f64,
    start_value2: f64,
    current_value: f64,
}

fn exploded_offset(center: Vec3, assembly_center: Vec3, factor: f32) -> Vec3 {
    (center - assembly_center) * factor.clamp(0.0, 1.0) * 1.5
}

const AMBIGUITY_DIAGONAL_FRACTION: f32 = 0.15;

#[derive(Clone, Copy, Debug, PartialEq)]
struct PickCandidate {
    item: SelItem,
    t: f32,
}

#[derive(Clone, Debug)]
struct PickPopup {
    candidates: Vec<PickCandidate>,
    position: Vec2,
    shift: bool,
}

fn nearest_face_per_body(hits: &[FaceHit]) -> Vec<FaceHit> {
    let mut bodies = HashSet::new();
    hits.iter()
        .copied()
        .filter(|hit| bodies.insert(hit.body))
        .collect()
}

fn ambiguous_candidates(
    candidates: &[PickCandidate],
    scene_diagonal: f32,
    edge_and_face: bool,
) -> Vec<PickCandidate> {
    if candidates.len() < 2 {
        return candidates.to_vec();
    }
    let close_faces = candidates
        .iter()
        .filter(|candidate| matches!(candidate.item, SelItem::Face(_, _) | SelItem::Body(_)))
        .map(|candidate| candidate.t)
        .take(2)
        .collect::<Vec<_>>();
    if edge_and_face
        || (close_faces.len() == 2
            && close_faces[1] - close_faces[0]
                < scene_diagonal.max(1.0e-4) * AMBIGUITY_DIAGONAL_FRACTION)
    {
        candidates.to_vec()
    } else {
        candidates.first().copied().into_iter().collect()
    }
}

fn resolve_selection_candidate(
    candidates: &[PickCandidate],
    select_second: bool,
) -> Option<PickCandidate> {
    candidates
        .get(usize::from(select_second))
        .or_else(|| candidates.first())
        .copied()
}

/// Full-window 3D canvas entity owned by the app root.
pub struct Viewport {
    renderer: Renderer,
    /// Shared document entity also read and mutated by the root chrome.
    pub document: Entity<Document>,
    _document_subscription: Subscription,
    scene_meshes: Vec<SceneMesh>,
    uploaded_epoch: u64,
    retessellate_only: Option<Vec<BodyId>>,
    hovered: Option<SelItem>,
    pick_popup: Option<PickPopup>,
    select_through: bool,
    camera: OrbitCamera,
    focus_handle: FocusHandle,
    current_rendered_frame: Option<Arc<RenderImage>>,
    previous_rendered_frame: Option<Arc<RenderImage>>,
    dirty: bool,
    dragging: Option<NavAction>,
    nav_preset: NavPreset,
    last_pointer: Vec2,
    last_tick: Instant,
    rendered_size: (u32, u32),
    show_grid: bool,
    display_mode: DisplayMode,
    visualize: bool,
    analysis: AnalysisMode,
    units: Units,
    /// Grid/endpoint snapping while sketching.
    snap_enabled: bool,
    /// Window backing-scale factor captured each frame (logical -> device px).
    device_scale: f32,
    hovered_gizmo: Option<Handle>,
    gizmo_drag: Option<GizmoDrag>,
    gizmo_repeat: usize,
    extrude_drag: Option<ExtrudeInteraction>,
    profile_extrude_drag: Option<ProfileExtrudeInteraction>,
    open_chain_extrude_drag: Option<OpenChainExtrudeInteraction>,
    dressup_drag: Option<DressUpInteraction>,
    variable_fillet: bool,
    shell_drag: Option<ShellInteraction>,
    thicken_drag: Option<ThickenInteraction>,
    revolve_interaction: Option<RevolveInteraction>,
    hole_interaction: Option<HoleInteraction>,
    draft_interaction: Option<DraftInteraction>,
    pattern_interaction: Option<PatternInteraction>,
    sketch_pattern_interaction: Option<SketchPatternInteraction>,
    helix_interaction: Option<HelixInteraction>,
    thread_interaction: Option<ThreadInteraction>,
    project_pending: bool,
    pending_reference: Option<PendingReference>,
    joint_tool_active: bool,
    joint_first: Option<(BodyId, Connector)>,
    joint_drive_enabled: bool,
    joint_drive: Option<JointDriveInteraction>,
    m6_interaction: Option<M6Interaction>,
    sketch_interaction: Option<SketchInteraction>,
    sketch_press: Option<Vec2>,
    sketch_entity_drag: Option<SketchEntityDrag>,
    numeric_input: Option<(Entity<NumericInput>, Vec2)>,
    numeric_input_subscription: Option<Subscription>,
    dimension_target: Option<(SketchId, SketchDimensionTarget)>,
    dimension_reference: bool,
    active_drag_tool: Option<ToolId>,
    extrude_mode: ExtrudeMode,
    extrude_side_mode: ExtrudeSideMode,
    hovered_extrude_arrow: bool,
    hovered_cube: Option<CubeRegion>,
    cube_interaction: Option<CubeInteraction>,
    marquee: Option<MarqueeInteraction>,
    section_enabled: bool,
    section_offset: Option<f32>,
    section_drag: Option<SectionDrag>,
    hovered_section_arrow: bool,
    section_interference_epoch: Option<u64>,
    isolated: Option<HashSet<BodyId>>,
    measure_enabled: bool,
    exploded_factor: f32,
    measure_anchors: Vec<Vec3>,
    /// Current delta label and its device-pixel viewport position.
    pub gizmo_readout: Option<(String, Vec2)>,
    /// Extrude mode chips and their device-pixel viewport anchor.
    pub extrude_badges: Option<(Vec<(ExtrudeMode, bool)>, Vec2)>,
    /// Extrude side-mode chips and their device-pixel viewport anchor.
    pub extrude_side_badges: Option<(Vec<(ExtrudeSideMode, bool)>, Vec2)>,
    theme: Theme,
}

impl Viewport {
    /// Initializes an empty renderer observing the shared document entity.
    pub fn new(
        document: Entity<Document>,
        theme: Theme,
        units: Units,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&document, |viewport, _, cx| {
            viewport.dirty = true;
            cx.notify();
        });
        let demo_section = std::env::var("FREE3D_DEMO_SCENE").is_ok_and(|scene| scene == "5");
        let mut viewport = Self {
            renderer: Renderer::new(theme.canvas).expect("failed to initialize the wgpu viewport"),
            document,
            _document_subscription: subscription,
            scene_meshes: Vec::new(),
            uploaded_epoch: u64::MAX,
            retessellate_only: None,
            hovered: None,
            pick_popup: None,
            select_through: false,
            camera: OrbitCamera::new(Vec3::new(0.0, 0.0, 25.0), 135.0, Vec2::new(1440.0, 900.0)),
            focus_handle: cx.focus_handle(),
            current_rendered_frame: None,
            previous_rendered_frame: None,
            dirty: true,
            dragging: None,
            nav_preset: NavPreset::default(),
            last_pointer: Vec2::ZERO,
            last_tick: Instant::now(),
            rendered_size: (0, 0),
            show_grid: true,
            display_mode: DisplayMode::Shaded,
            visualize: false,
            analysis: std::env::var("FREE3D_ANALYSIS")
                .is_ok_and(|value| value.eq_ignore_ascii_case("zebra"))
                .then_some(AnalysisMode::Zebra)
                .unwrap_or_default(),
            units,
            snap_enabled: true,
            device_scale: 2.0,
            hovered_gizmo: None,
            gizmo_drag: None,
            gizmo_repeat: 1,
            extrude_drag: None,
            profile_extrude_drag: None,
            open_chain_extrude_drag: None,
            dressup_drag: None,
            variable_fillet: false,
            shell_drag: None,
            thicken_drag: None,
            revolve_interaction: None,
            hole_interaction: None,
            draft_interaction: None,
            pattern_interaction: None,
            sketch_pattern_interaction: None,
            helix_interaction: None,
            thread_interaction: None,
            project_pending: false,
            pending_reference: None,
            joint_tool_active: false,
            joint_first: None,
            joint_drive_enabled: false,
            joint_drive: None,
            m6_interaction: None,
            sketch_interaction: None,
            sketch_press: None,
            sketch_entity_drag: None,
            numeric_input: None,
            numeric_input_subscription: None,
            dimension_target: None,
            dimension_reference: false,
            active_drag_tool: None,
            extrude_mode: ExtrudeMode::Auto,
            extrude_side_mode: ExtrudeSideMode::OneSided,
            hovered_extrude_arrow: false,
            hovered_cube: None,
            cube_interaction: None,
            marquee: None,
            section_enabled: demo_section,
            section_offset: None,
            section_drag: None,
            hovered_section_arrow: false,
            section_interference_epoch: None,
            isolated: None,
            measure_enabled: false,
            exploded_factor: 0.0,
            measure_anchors: Vec::new(),
            gizmo_readout: None,
            extrude_badges: None,
            extrude_side_badges: None,
            theme,
        };
        viewport.sync_scene(cx);
        viewport.renderer.set_analysis(viewport.analysis);
        if demo_section {
            viewport.ensure_section_offset();
        }
        if std::env::var_os("FREE3D_DUMP_FRAME").is_some() {
            let selection = viewport.document.read(cx).selection.items.clone();
            let gizmo = viewport.gizmo_state(cx);
            let extrude_arrow = viewport.tool_arrow_state(cx);
            let rendered = viewport.renderer.render(
                &viewport.camera,
                1440,
                900,
                viewport.show_grid,
                viewport.display_mode,
                viewport.analysis,
                viewport.visualize,
                viewport.hovered,
                &selection,
                gizmo,
                extrude_arrow,
                viewport.section_arrow_state(),
                viewport.section_plane(),
                OrientationCubeRender {
                    device_scale: viewport.device_scale,
                    hovered: viewport.hovered_cube,
                },
            );
            Self::dump_frame(&rendered);
        }
        viewport
    }

    /// Updates the design tokens used by viewport-owned floating controls.
    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Updates the renderer palette and invalidates the current canvas frame.
    pub fn set_canvas_theme(&mut self, canvas: crate::theme::CanvasTheme, cx: &mut Context<Self>) {
        self.renderer.set_canvas_theme(canvas);
        self.dirty = true;
        cx.notify();
    }

    /// Focuses the viewport for navigation and modeling shortcuts.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    /// Updates the gesture table used by subsequent navigation input.
    pub fn set_nav_preset(&mut self, preset: NavPreset, cx: &mut Context<Self>) {
        self.nav_preset = preset;
        cx.notify();
    }

    /// Captures the current camera values for a session saved view.
    pub fn saved_view(&self) -> SavedView {
        SavedView {
            pivot: self.camera.pivot,
            yaw: self.camera.yaw,
            pitch: self.camera.pitch,
            distance: self.camera.distance,
            fov_degrees: self.camera.fov.to_degrees(),
        }
    }

    /// Animates to a saved camera state, including its pivot and FOV.
    pub fn recall_saved_view(
        &mut self,
        view: SavedView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.camera.fov = view.fov_degrees.clamp(5.0, 90.0).to_radians();
        self.camera
            .animate_to_pivot(view.pivot, view.yaw, view.pitch, view.distance);
        self.changed(window, cx);
    }

    /// Snaps the camera to a standard orientation, preserving the current zoom.
    pub fn go_to_standard_view(
        &mut self,
        view: StandardView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (yaw, pitch) = view.orientation();
        self.camera.animate_to(yaw, pitch, self.camera.distance);
        self.changed(window, cx);
    }

    /// Sets the vertical field of view in degrees (clamped to a sane range).
    pub fn set_fov(&mut self, degrees: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.camera.fov = degrees.clamp(5.0, 90.0).to_radians();
        self.changed(window, cx);
    }

    /// Enables or disables sketch snapping (grid rounding + endpoint capture).
    pub fn set_snap_enabled(&mut self, enabled: bool) {
        self.snap_enabled = enabled;
    }

    /// Current adaptive grid pitch in model units (mm), for the HUD indicator.
    pub fn grid_pitch(&self) -> f32 {
        grid::adaptive_pitch(self.camera.distance)
    }

    /// Applies a body/edge display mode.
    pub fn set_display_mode(
        &mut self,
        display_mode: DisplayMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.display_mode != display_mode {
            self.display_mode = display_mode;
            self.changed(window, cx);
        }
    }

    /// Shows one read-only common shape as a translucent red overlay.
    pub fn show_interference_shape(&mut self, shape: Option<&occt::Shape>, cx: &mut Context<Self>) {
        let preview = shape.map(|shape| (tessellate(shape, 0.2), Mat4::IDENTITY));
        self.renderer.set_interference_mesh(preview);
        self.dirty = true;
        cx.notify();
    }

    /// Applies a mutually exclusive surface-analysis mode.
    pub fn set_analysis(&mut self, analysis: AnalysisMode, cx: &mut Context<Self>) {
        self.analysis = analysis;
        self.renderer.set_analysis(analysis);
        self.dirty = true;
        cx.notify();
    }

    /// Changes readout and numeric-entry units without changing model geometry.
    pub fn set_units(&mut self, units: Units, cx: &mut Context<Self>) {
        self.units = units;
        cx.notify();
    }

    /// Toggles ground-plane grid visibility in the rendered scene.
    pub fn set_grid_visible(&mut self, visible: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_grid != visible {
            self.show_grid = visible;
            self.changed(window, cx);
        }
    }

    /// Enables the richer material-lighting presentation used by Visualize space.
    pub fn set_visualize(&mut self, visualize: bool, cx: &mut Context<Self>) {
        if self.visualize != visualize {
            self.visualize = visualize;
            self.dirty = true;
            cx.notify();
        }
    }

    /// Updates grid visibility for a workspace transition without a window handle.
    pub fn set_grid_visible_passive(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.show_grid != visible {
            self.show_grid = visible;
            self.dirty = true;
            cx.notify();
        }
    }

    /// Activates or deactivates a view-only bottom-left interaction mode.
    pub fn set_mode(
        &mut self,
        mode: ModeChip,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match mode {
            ModeChip::Section => {
                self.section_enabled = active;
                self.section_drag = None;
                self.hovered_section_arrow = false;
                if active {
                    self.ensure_section_offset();
                    self.refresh_section_interference(cx);
                } else {
                    self.section_interference_epoch = None;
                    self.renderer.set_interference_mesh(None);
                }
            }
            ModeChip::Isolate => {
                self.isolated = active.then(|| {
                    self.document
                        .read(cx)
                        .selection
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            SelItem::Body(id) => Some(*id),
                            _ => None,
                        })
                        .collect()
                });
                self.uploaded_epoch = u64::MAX;
                self.sync_scene(cx);
            }
            ModeChip::Measure => {
                self.measure_enabled = active;
                self.measure_anchors.clear();
                self.renderer.set_measure_line(None);
            }
            ModeChip::Exploded => {
                self.exploded_factor = if active { 1.0 } else { 0.0 };
                self.uploaded_epoch = u64::MAX;
                self.sync_scene(cx);
            }
        }
        self.changed(window, cx);
    }

    /// Sets the continuous assembly exploded-view factor.
    pub fn set_exploded_factor(
        &mut self,
        factor: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exploded_factor = factor.clamp(0.0, 1.0);
        self.uploaded_epoch = u64::MAX;
        self.sync_scene(cx);
        self.changed(window, cx);
    }

    fn exit_modes(&mut self) -> bool {
        let active = self.section_enabled
            || self.isolated.is_some()
            || self.measure_enabled
            || self.exploded_factor > 0.0;
        if !active {
            return false;
        }
        self.section_enabled = false;
        self.section_drag = None;
        self.hovered_section_arrow = false;
        self.isolated = None;
        self.measure_enabled = false;
        self.exploded_factor = 0.0;
        self.measure_anchors.clear();
        self.renderer.set_measure_line(None);
        self.uploaded_epoch = u64::MAX;
        active
    }

    fn scene_bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        for body in self.scene_meshes.iter().filter(|body| {
            body.visible
                && self
                    .isolated
                    .as_ref()
                    .is_none_or(|isolated| isolated.contains(&body.id))
        }) {
            for &position in &body.mesh.positions {
                let position = Vec3::from(position);
                minimum = minimum.min(position);
                maximum = maximum.max(position);
            }
        }
        minimum.is_finite().then_some((minimum, maximum))
    }

    fn ensure_section_offset(&mut self) {
        if self.section_offset.is_none()
            && let Some((minimum, maximum)) = self.scene_bounds()
        {
            self.section_offset = Some((minimum.y + maximum.y) * 0.5);
        }
    }

    fn section_plane(&self) -> Option<[f32; 4]> {
        // Clipping remains shader-only; the renderer adds a documented
        // inward-shell cap approximation for closed solids.
        self.section_enabled
            .then_some(self.section_offset)
            .flatten()
            .map(|offset| {
                debug_assert!(!point_is_clipped(
                    SECTION_NORMAL * offset,
                    SECTION_NORMAL,
                    offset,
                ));
                [SECTION_NORMAL.x, SECTION_NORMAL.y, SECTION_NORMAL.z, offset]
            })
    }

    fn section_arrow_state(&self) -> Option<ExtrudeArrowRender> {
        let offset = self
            .section_enabled
            .then_some(self.section_offset)
            .flatten()?;
        let (minimum, maximum) = self.scene_bounds()?;
        let origin = Vec3::new(
            (minimum.x + maximum.x) * 0.5,
            offset,
            (minimum.z + maximum.z) * 0.5,
        );
        Some(ExtrudeArrowRender {
            origin,
            normal: SECTION_NORMAL,
            scale: self.gizmo_scale(origin),
            hovered: self.hovered_section_arrow || self.section_drag.is_some(),
        })
    }

    fn begin_section_drag(&mut self, pointer: Vec2) -> bool {
        let Some(arrow) = self.section_arrow_state() else {
            return false;
        };
        let ray = self.camera.unproject_ray(pointer);
        if !hit_test_axis(ray, arrow.origin, arrow.normal, arrow.scale) {
            return false;
        }
        self.section_drag = Some(SectionDrag {
            origin: arrow.origin,
            start_offset: self.section_offset.expect("section arrow has an offset"),
            start_parameter: axis_drag_parameter(ray.0, ray.1, arrow.origin, arrow.normal),
        });
        true
    }

    fn update_section_drag(&mut self, pointer: Vec2) {
        let Some(drag) = &self.section_drag else {
            return;
        };
        let origin = drag.origin;
        let start_offset = drag.start_offset;
        let start_parameter = drag.start_parameter;
        let ray = self.camera.unproject_ray(pointer);
        let parameter = axis_drag_parameter(ray.0, ray.1, origin, SECTION_NORMAL);
        self.section_offset = Some(start_offset + parameter - start_parameter);
    }

    fn measure_anchor_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<Vec3> {
        let filter = self.document.read(cx).selection.filter;
        let item = self.item_at(pointer, false, filter)?;
        let body_id = item.body_id()?;
        let body = self.scene_meshes.iter().find(|body| body.id == body_id)?;
        match item {
            SelItem::Edge(_, edge_index) => {
                let edge = body.mesh.edges.get(edge_index as usize)?;
                edge.points
                    .windows(2)
                    .filter_map(|segment| {
                        let world_a = Vec3::from(segment[0]);
                        let world_b = Vec3::from(segment[1]);
                        let a = self.camera.project(world_a);
                        let b = self.camera.project(world_b);
                        let line = b - a;
                        let t = if line.length_squared() <= f32::EPSILON {
                            0.0
                        } else {
                            ((pointer - a).dot(line) / line.length_squared()).clamp(0.0, 1.0)
                        };
                        let projected = a + line * t;
                        projected.is_finite().then_some((
                            pointer.distance_squared(projected),
                            world_a.lerp(world_b, t),
                        ))
                    })
                    .min_by(|left, right| left.0.total_cmp(&right.0))
                    .map(|(_, point)| point)
            }
            SelItem::Face(_, face_index) => {
                let range = body.mesh.face_ranges.get(face_index as usize)?;
                let mut count = 0.0;
                let center = body.mesh.indices[range.start as usize..range.end as usize]
                    .iter()
                    .fold(Vec3::ZERO, |center, &index| {
                        count += 1.0;
                        center + Vec3::from(body.mesh.positions[index as usize])
                    });
                (count > 0.0).then_some(center / count)
            }
            SelItem::Body(_) => {
                let (minimum, maximum) = body.mesh.positions.iter().fold(
                    (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
                    |(minimum, maximum), &position| {
                        let position = Vec3::from(position);
                        (minimum.min(position), maximum.max(position))
                    },
                );
                minimum.is_finite().then_some((minimum + maximum) * 0.5)
            }
            SelItem::Profile(_, _)
            | SelItem::SketchEntity(_, _)
            | SelItem::Plane(_)
            | SelItem::Axis(_)
            | SelItem::Point(_) => None,
        }
    }

    fn add_measure_anchor(&mut self, pointer: Vec2, cx: &Context<Self>) {
        let Some(anchor) = self.measure_anchor_at(pointer, cx) else {
            return;
        };
        if self.measure_anchors.len() == 2 {
            self.measure_anchors.clear();
        }
        self.measure_anchors.push(anchor);
        let line = (self.measure_anchors.len() == 2)
            .then(|| [self.measure_anchors[0], self.measure_anchors[1]]);
        self.renderer.set_measure_line(line);
    }

    /// Enters plane-local sketch editing with one of the basic drawing tools.
    pub fn enter_sketch(
        &mut self,
        id: SketchId,
        plane: SketchPlane,
        tool: ToolId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sketch_interaction = Some(SketchInteraction {
            id,
            plane,
            tool,
            anchor: None,
            anchor_ref: None,
            arc_end: None,
            arc_end_ref: None,
            chain_start: None,
            cursor: None,
            hv_snapped: false,
            fillet_first: None,
            fillet_pair: None,
            edit_profile: None,
            edit_distance: 0.0,
            trim_preview: None,
            spline_points: Vec::new(),
            spline_start_ref: None,
            spline_end_ref: None,
            tangent_arc: tool == ToolId::TangentArc,
            chain_tangent: None,
            aux_point: None,
            phase_value: None,
            polygon_sides: 6,
        });
        self.renderer.set_grid_plane(Some(plane));
        let forward = -plane.normal().as_vec3();
        let yaw = forward.y.atan2(forward.x);
        let pitch = forward.z.asin();
        // TODO(M4c): OrbitCamera is constrained to world-Z-up, so arbitrary
        // face planes cannot also preserve plane.y_axis as screen-up.
        self.camera
            .animate_to_pivot(plane.origin.as_vec3(), yaw, pitch, self.camera.distance);
        self.sync_sketch_gpu(cx);
        self.changed(window, cx);
    }

    fn exit_sketch(&mut self, cx: &mut Context<Self>) {
        self.sketch_interaction = None;
        self.renderer.set_grid_plane(None);
        self.document.update(cx, |document, cx| {
            document.finish_sketch_mode();
            cx.notify();
        });
        self.sync_sketch_gpu(cx);
    }

    /// Resolves the sketch the pointer should interact with: the active tool
    /// interaction if any, else the document's active sketch (profiles stay
    /// clickable after a drawing tool exits, matching the Shapr3D flow).
    fn active_sketch_context(&self, cx: &Context<Self>) -> Option<(SketchId, SketchPlane)> {
        if let Some(interaction) = self.sketch_interaction.as_ref() {
            return Some((interaction.id, interaction.plane));
        }
        let document = self.document.read(cx);
        let id = document.active_sketch?;
        let sketch = document.sketches.iter().find(|sketch| sketch.id == id)?;
        Some((id, sketch.plane))
    }

    fn sketch_local_at(&self, pointer: Vec2) -> Option<glam::DVec2> {
        let interaction = self.sketch_interaction.as_ref()?;
        self.sketch_local_on_plane(pointer, &interaction.plane)
    }

    fn sketch_local_on_plane(&self, pointer: Vec2, plane: &SketchPlane) -> Option<glam::DVec2> {
        let (origin, direction) = self.camera.unproject_ray(pointer);
        let normal = plane.normal().as_vec3();
        let denominator = direction.dot(normal);
        if denominator.abs() < 1.0e-6 {
            return None;
        }
        let distance = (plane.origin.as_vec3() - origin).dot(normal) / denominator;
        (distance >= 0.0).then(|| plane.to_local((origin + direction * distance).as_dvec3()))
    }

    fn snapped_sketch_point(
        &self,
        pointer: Vec2,
        cx: &Context<Self>,
    ) -> Option<(glam::DVec2, bool, Option<PointRef>)> {
        let interaction = self.sketch_interaction.as_ref()?;
        let mut point = self.sketch_local_at(pointer)?;
        if self.snap_enabled {
            let pitch = f64::from(grid::adaptive_pitch(self.camera.distance)) / 10.0;
            point = (point / pitch).round() * pitch;
        }
        let mut endpoint_ref = None;
        if self.snap_enabled
            && let Some(sketch) = self
                .document
                .read(cx)
                .sketches
                .iter()
                .find(|sketch| sketch.id == interaction.id)
        {
            let endpoint = sketch
                .entities
                .iter()
                .enumerate()
                .flat_map(|(entity, curve)| match &curve.geo {
                    SketchEntity::Line { a, b } => vec![
                        (*a, PointRef { entity, point: 0 }),
                        (*b, PointRef { entity, point: 1 }),
                    ],
                    SketchEntity::Circle { .. } | SketchEntity::Ellipse { .. } => Vec::new(),
                    SketchEntity::Arc { start, end, .. } => vec![
                        (*start, PointRef { entity, point: 0 }),
                        (*end, PointRef { entity, point: 1 }),
                    ],
                    SketchEntity::Spline { points } if points.len() >= 2 => vec![
                        (points[0], PointRef { entity, point: 0 }),
                        (
                            *points.last().expect("spline endpoint"),
                            PointRef { entity, point: 1 },
                        ),
                    ],
                    SketchEntity::CvSpline { control, .. } if control.len() >= 2 => vec![
                        (control[0], PointRef { entity, point: 0 }),
                        (
                            *control.last().expect("CV spline endpoint"),
                            PointRef { entity, point: 1 },
                        ),
                    ],
                    SketchEntity::EllipseArc {
                        center,
                        major,
                        minor_ratio,
                        start_angle,
                        end_angle,
                    } => vec![
                        (
                            ellipse_point(*center, *major, *minor_ratio, *start_angle),
                            PointRef { entity, point: 0 },
                        ),
                        (
                            ellipse_point(*center, *major, *minor_ratio, *end_angle),
                            PointRef { entity, point: 1 },
                        ),
                    ],
                    SketchEntity::Point { at } => vec![(*at, PointRef { entity, point: 0 })],
                    SketchEntity::Spline { .. } | SketchEntity::CvSpline { .. } => Vec::new(),
                })
                .filter_map(|(candidate, reference)| {
                    let screen = self
                        .camera
                        .project(interaction.plane.to_world(candidate).as_vec3());
                    (screen.distance(pointer) <= 8.0 * self.device_scale.max(1.0)).then_some((
                        screen.distance(pointer),
                        candidate,
                        reference,
                    ))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((_, endpoint, reference)) = endpoint {
                point = endpoint;
                endpoint_ref = Some(reference);
            }
        }
        let Some(anchor) = interaction.anchor else {
            return Some((point, false, endpoint_ref));
        };
        let before_hv = point;
        let (point, snapped) = if matches!(interaction.tool, ToolId::Spline | ToolId::CvSpline) {
            (point, false)
        } else {
            snap_horizontal_vertical(anchor, point, 3.0)
        };
        if point != before_hv {
            endpoint_ref = None;
        }
        Some((point, snapped, endpoint_ref))
    }

    fn pending_sketch_lines(&self) -> Vec<(Vec3, Vec3)> {
        if let Some(thread) = &self.thread_interaction {
            let axis = thread.axis.normalize_or_zero();
            let reference = if axis.x.abs() < 0.8 {
                DVec3::X
            } else {
                DVec3::Y
            };
            let x = axis.cross(reference).normalize_or_zero();
            let y = axis.cross(x).normalize_or_zero();
            let turns = thread.depth / thread.pitch.max(1.0e-6);
            let segments = (turns * 48.0).ceil().max(8.0) as usize;
            let points: Vec<_> = (0..=segments)
                .map(|index| {
                    let t = index as f64 / segments as f64;
                    let angle = std::f64::consts::TAU * turns * t;
                    (thread.origin
                        + axis * (thread.depth * t)
                        + (x * angle.cos() + y * angle.sin()) * thread.radius)
                        .as_vec3()
                })
                .collect();
            return points.windows(2).map(|pair| (pair[0], pair[1])).collect();
        }
        let Some(interaction) = &self.sketch_interaction else {
            return Vec::new();
        };
        let world = |point| interaction.plane.to_world(point).as_vec3();
        if let Some(preview) = &interaction.trim_preview {
            return match preview {
                SketchEntity::Line { a, b } => vec![(world(*a), world(*b))],
                SketchEntity::Arc { start, end, mid } => sample_arc(*start, *mid, *end, 32)
                    .windows(2)
                    .map(|pair| (world(pair[0]), world(pair[1])))
                    .collect(),
                SketchEntity::Circle { center, radius } => (0..64)
                    .map(|segment| {
                        let angle = |index: usize| index as f64 / 64.0 * std::f64::consts::TAU;
                        (
                            world(
                                *center
                                    + glam::DVec2::new(angle(segment).cos(), angle(segment).sin())
                                        * *radius,
                            ),
                            world(
                                *center
                                    + glam::DVec2::new(
                                        angle(segment + 1).cos(),
                                        angle(segment + 1).sin(),
                                    ) * *radius,
                            ),
                        )
                    })
                    .collect(),
                SketchEntity::Ellipse {
                    center,
                    major,
                    minor_ratio,
                } => sample_ellipse(*center, *major, *minor_ratio, 64)
                    .windows(2)
                    .map(|pair| (world(pair[0]), world(pair[1])))
                    .collect(),
                SketchEntity::Spline { points } => sample_spline(points, interaction.plane)
                    .windows(2)
                    .map(|pair| (world(pair[0]), world(pair[1])))
                    .collect(),
                SketchEntity::CvSpline { control, degree } => {
                    crate::sketch::sample_cv_spline(control, *degree, interaction.plane)
                        .windows(2)
                        .map(|pair| (world(pair[0]), world(pair[1])))
                        .collect()
                }
                SketchEntity::EllipseArc {
                    center,
                    major,
                    minor_ratio,
                    start_angle,
                    end_angle,
                } => {
                    sample_ellipse_arc(*center, *major, *minor_ratio, *start_angle, *end_angle, 32)
                        .windows(2)
                        .map(|pair| (world(pair[0]), world(pair[1])))
                        .collect()
                }
                SketchEntity::Point { at } => {
                    let d = 0.8;
                    vec![
                        (
                            world(*at - glam::DVec2::X * d),
                            world(*at + glam::DVec2::X * d),
                        ),
                        (
                            world(*at - glam::DVec2::Y * d),
                            world(*at + glam::DVec2::Y * d),
                        ),
                    ]
                }
            };
        }
        let (Some(anchor), Some(cursor)) = (interaction.anchor, interaction.cursor) else {
            return Vec::new();
        };
        match interaction.tool {
            ToolId::Line | ToolId::TangentArc => {
                if interaction.tangent_arc
                    && let Some(tangent) = interaction.chain_tangent
                    && let Some(mid) = tangent_arc_mid(anchor, tangent, cursor)
                {
                    sample_arc(anchor, mid, cursor, 32)
                        .windows(2)
                        .map(|pair| (world(pair[0]), world(pair[1])))
                        .collect()
                } else {
                    vec![(world(anchor), world(cursor))]
                }
            }
            ToolId::Rectangle | ToolId::CenterRectangle => {
                let (a, c) = if interaction.tool == ToolId::CenterRectangle {
                    let half = (cursor - anchor).abs();
                    (anchor - half, anchor + half)
                } else {
                    (anchor, cursor)
                };
                let b = glam::DVec2::new(c.x, a.y);
                let d = glam::DVec2::new(a.x, c.y);
                vec![
                    (world(a), world(b)),
                    (world(b), world(c)),
                    (world(c), world(d)),
                    (world(d), world(a)),
                ]
            }
            ToolId::Polygon => regular_polygon(anchor, cursor, interaction.polygon_sides)
                .map(|(entities, _)| {
                    entities
                        .into_iter()
                        .filter_map(|entity| match entity {
                            SketchEntity::Line { a, b } => Some((world(a), world(b))),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            ToolId::Slot | ToolId::RoundedRectangle => {
                let Some(second) = interaction.arc_end else {
                    return vec![(world(anchor), world(cursor))];
                };
                let radius = if interaction.tool == ToolId::Slot {
                    let axis = (second - anchor).normalize_or_zero();
                    (cursor - second).perp_dot(axis).abs()
                } else {
                    cursor.distance(second).max(1.0e-6)
                };
                let generated = if interaction.tool == ToolId::Slot {
                    slot(anchor, second, radius)
                } else {
                    rounded_rectangle(anchor, second, radius)
                };
                generated
                    .map(|(entities, _)| {
                        entities
                            .into_iter()
                            .flat_map(primitive_segments)
                            .map(|(a, b)| (world(a), world(b)))
                            .collect()
                    })
                    .unwrap_or_default()
            }
            ToolId::Circle => {
                let radius = anchor.distance(cursor);
                const SEGMENTS: usize = 64;
                (0..SEGMENTS)
                    .map(|segment| {
                        let angle =
                            |index: usize| index as f64 / SEGMENTS as f64 * std::f64::consts::TAU;
                        let a = anchor
                            + glam::DVec2::new(angle(segment).cos(), angle(segment).sin()) * radius;
                        let b = anchor
                            + glam::DVec2::new(angle(segment + 1).cos(), angle(segment + 1).sin())
                                * radius;
                        (world(a), world(b))
                    })
                    .collect()
            }
            ToolId::ThreePointCircle => {
                let Some(second) = interaction.arc_end else {
                    return vec![(world(anchor), world(cursor))];
                };
                let Some((center, radius)) = three_point_circle(anchor, second, cursor) else {
                    return vec![
                        (world(anchor), world(second)),
                        (world(second), world(cursor)),
                    ];
                };
                (0..64)
                    .map(|segment| {
                        let a = std::f64::consts::TAU * segment as f64 / 64.0;
                        let b = std::f64::consts::TAU * (segment + 1) as f64 / 64.0;
                        (
                            world(center + glam::DVec2::new(a.cos(), a.sin()) * radius),
                            world(center + glam::DVec2::new(b.cos(), b.sin()) * radius),
                        )
                    })
                    .collect()
            }
            ToolId::Ellipse => {
                let Some(major_end) = interaction.arc_end else {
                    return vec![(world(anchor), world(cursor))];
                };
                let major = major_end - anchor;
                let ratio = if major.length_squared() <= 1.0e-12 {
                    0.0
                } else {
                    let perpendicular = glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                    ((cursor - anchor).dot(perpendicular).abs() / major.length()).clamp(0.0, 1.0)
                };
                sample_ellipse(anchor, major, ratio, 64)
                    .windows(2)
                    .map(|pair| (world(pair[0]), world(pair[1])))
                    .collect()
            }
            ToolId::EllipseArc => {
                let Some(major_end) = interaction.arc_end else {
                    return vec![(world(anchor), world(cursor))];
                };
                let major = major_end - anchor;
                let ratio = interaction.phase_value.unwrap_or_else(|| {
                    let normal = glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                    ((cursor - anchor).dot(normal).abs() / major.length()).clamp(1.0e-6, 1.0)
                });
                if let Some(start) = interaction.aux_point {
                    let parameter = |p: glam::DVec2| {
                        let x = (p - anchor).dot(major.normalize_or_zero()) / major.length();
                        let normal = glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                        ((p - anchor).dot(normal) / (major.length() * ratio)).atan2(x)
                    };
                    let a = parameter(start);
                    let mut b = parameter(cursor);
                    if b <= a {
                        b += std::f64::consts::TAU;
                    }
                    sample_ellipse_arc(anchor, major, ratio, a, b, 32)
                        .windows(2)
                        .map(|p| (world(p[0]), world(p[1])))
                        .collect()
                } else {
                    sample_ellipse(anchor, major, ratio, 64)
                        .windows(2)
                        .map(|p| (world(p[0]), world(p[1])))
                        .collect()
                }
            }
            ToolId::Arc => {
                let Some(end) = interaction.arc_end else {
                    return vec![(world(anchor), world(cursor))];
                };
                sample_arc(anchor, cursor, end, 32)
                    .windows(2)
                    .map(|pair| (world(pair[0]), world(pair[1])))
                    .collect()
            }
            ToolId::Spline | ToolId::CvSpline => {
                let mut points = interaction.spline_points.clone();
                if points
                    .last()
                    .is_none_or(|last| last.distance(cursor) > 1.0e-9)
                {
                    points.push(cursor);
                }
                let sampled = if interaction.tool == ToolId::CvSpline {
                    crate::sketch::sample_cv_spline(&points, 3, interaction.plane)
                } else {
                    sample_spline(&points, interaction.plane)
                };
                sampled
                    .windows(2)
                    .map(|pair| (world(pair[0]), world(pair[1])))
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    fn sync_sketch_gpu(&mut self, cx: &Context<Self>) {
        let pending = self.pending_sketch_lines();
        let document = self.document.read(cx);
        self.renderer.upload_sketches(
            &document.sketches,
            &document.construction_planes,
            &document.construction_axes,
            &document.construction_points,
            &pending,
            &document.selection.items,
        );
        self.renderer
            .upload_reference_images(&document.reference_images);
    }

    fn profile_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<SelItem> {
        // Never pick profiles mid-draw (an anchored tool action is running).
        if let Some(interaction) = self.sketch_interaction.as_ref()
            && interaction.anchor.is_some()
        {
            return None;
        }
        let (id, plane) = self.active_sketch_context(cx)?;
        let point = self.sketch_local_on_plane(pointer, &plane)?;
        let sketch = self
            .document
            .read(cx)
            .sketches
            .iter()
            .find(|sketch| sketch.id == id)?;
        sketch
            .profiles()
            .iter()
            .enumerate()
            .rev()
            .find(|(_, profile)| profile_contains(sketch, profile, point))
            .map(|(index, _)| SelItem::Profile(id, index))
    }

    fn sketch_entity_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<SelItem> {
        let interaction = self.sketch_interaction.as_ref()?;
        if interaction.anchor.is_some() {
            return None;
        }
        let sketch = self
            .document
            .read(cx)
            .sketches
            .iter()
            .find(|sketch| sketch.id == interaction.id)?;
        let threshold = 6.0 * self.device_scale.max(1.0);
        sketch
            .entities
            .iter()
            .enumerate()
            .filter_map(|(index, entity)| {
                let distance = match &entity.geo {
                    SketchEntity::Line { a, b } => {
                        let a = self.camera.project(sketch.plane.to_world(*a).as_vec3());
                        let b = self.camera.project(sketch.plane.to_world(*b).as_vec3());
                        point_segment_distance(pointer, a, b)
                    }
                    SketchEntity::Circle { center, radius } => {
                        const SEGMENTS: usize = 64;
                        (0..SEGMENTS)
                            .map(|segment| {
                                let angle = |index: usize| {
                                    index as f64 / SEGMENTS as f64 * std::f64::consts::TAU
                                };
                                let screen = |angle: f64| {
                                    self.camera.project(
                                        sketch
                                            .plane
                                            .to_world(
                                                *center
                                                    + glam::DVec2::new(angle.cos(), angle.sin())
                                                        * *radius,
                                            )
                                            .as_vec3(),
                                    )
                                };
                                point_segment_distance(
                                    pointer,
                                    screen(angle(segment)),
                                    screen(angle(segment + 1)),
                                )
                            })
                            .fold(f32::INFINITY, f32::min)
                    }
                    SketchEntity::Ellipse {
                        center,
                        major,
                        minor_ratio,
                    } => sample_ellipse(*center, *major, *minor_ratio, 64)
                        .windows(2)
                        .map(|pair| {
                            point_segment_distance(
                                pointer,
                                self.camera
                                    .project(sketch.plane.to_world(pair[0]).as_vec3()),
                                self.camera
                                    .project(sketch.plane.to_world(pair[1]).as_vec3()),
                            )
                        })
                        .fold(f32::INFINITY, f32::min),
                    SketchEntity::Arc { start, end, mid } => sample_arc(*start, *mid, *end, 32)
                        .windows(2)
                        .map(|pair| {
                            point_segment_distance(
                                pointer,
                                self.camera
                                    .project(sketch.plane.to_world(pair[0]).as_vec3()),
                                self.camera
                                    .project(sketch.plane.to_world(pair[1]).as_vec3()),
                            )
                        })
                        .fold(f32::INFINITY, f32::min),
                    SketchEntity::Spline { points } => sketch
                        .spline_polyline(index, points)
                        .windows(2)
                        .map(|pair| {
                            point_segment_distance(
                                pointer,
                                self.camera
                                    .project(sketch.plane.to_world(pair[0]).as_vec3()),
                                self.camera
                                    .project(sketch.plane.to_world(pair[1]).as_vec3()),
                            )
                        })
                        .fold(f32::INFINITY, f32::min),
                    SketchEntity::CvSpline { control, degree } => {
                        crate::sketch::sample_cv_spline(control, *degree, sketch.plane)
                            .windows(2)
                            .map(|pair| {
                                point_segment_distance(
                                    pointer,
                                    self.camera
                                        .project(sketch.plane.to_world(pair[0]).as_vec3()),
                                    self.camera
                                        .project(sketch.plane.to_world(pair[1]).as_vec3()),
                                )
                            })
                            .fold(f32::INFINITY, f32::min)
                    }
                    SketchEntity::EllipseArc {
                        center,
                        major,
                        minor_ratio,
                        start_angle,
                        end_angle,
                    } => sample_ellipse_arc(
                        *center,
                        *major,
                        *minor_ratio,
                        *start_angle,
                        *end_angle,
                        32,
                    )
                    .windows(2)
                    .map(|pair| {
                        point_segment_distance(
                            pointer,
                            self.camera
                                .project(sketch.plane.to_world(pair[0]).as_vec3()),
                            self.camera
                                .project(sketch.plane.to_world(pair[1]).as_vec3()),
                        )
                    })
                    .fold(f32::INFINITY, f32::min),
                    SketchEntity::Point { at } => {
                        pointer.distance(self.camera.project(sketch.plane.to_world(*at).as_vec3()))
                    }
                };
                (distance <= threshold).then_some((distance, index))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, index)| SelItem::SketchEntity(interaction.id, index))
    }

    fn sketch_mouse_down(
        &mut self,
        pointer: Vec2,
        double_click: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((point, hv_snapped, endpoint_ref)) = self.snapped_sketch_point(pointer, cx) else {
            return false;
        };
        if self
            .sketch_interaction
            .as_ref()
            .is_some_and(|interaction| interaction.anchor.is_none())
            && let Some(profile) = self.profile_at(pointer, cx)
        {
            self.document.update(cx, |document, cx| {
                document.selection.apply(profile, false);
                cx.notify();
            });
            return true;
        }
        let interaction = self.sketch_interaction.as_mut().expect("checked above");
        if double_click && matches!(interaction.tool, ToolId::Line | ToolId::TangentArc) {
            interaction.anchor = None;
            interaction.anchor_ref = None;
            interaction.chain_start = None;
            interaction.cursor = None;
            self.sync_sketch_gpu(cx);
            return true;
        }
        match interaction.tool {
            ToolId::Spline | ToolId::CvSpline => {
                if interaction.spline_points.is_empty() {
                    interaction.anchor = Some(point);
                    interaction.spline_start_ref = endpoint_ref;
                }
                if interaction
                    .spline_points
                    .last()
                    .is_none_or(|last| last.distance(point) > 1.0e-9)
                {
                    interaction.spline_points.push(point);
                }
                interaction.spline_end_ref = endpoint_ref;
                interaction.cursor = Some(point);
                if double_click && interaction.spline_points.len() >= 2 {
                    let id = interaction.id;
                    let points = std::mem::take(&mut interaction.spline_points);
                    let start_ref = interaction.spline_start_ref.take();
                    let end_ref = interaction.spline_end_ref.take();
                    interaction.anchor = None;
                    interaction.cursor = None;
                    self.document.update(cx, |document, cx| {
                        let index = document
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == id)
                            .map_or(0, |sketch| sketch.entities.len());
                        let mut constraints = Vec::new();
                        if let Some(existing) = start_ref {
                            constraints.push(Constraint::Coincident {
                                a: PointRef {
                                    entity: index,
                                    point: 0,
                                },
                                b: existing,
                            });
                        }
                        if let Some(existing) = end_ref {
                            constraints.push(Constraint::Coincident {
                                a: PointRef {
                                    entity: index,
                                    point: 1,
                                },
                                b: existing,
                            });
                        }
                        let geo = if interaction.tool == ToolId::CvSpline {
                            SketchEntity::CvSpline {
                                control: points,
                                degree: 3,
                            }
                        } else {
                            SketchEntity::Spline { points }
                        };
                        document.add_sketch_entities_with_constraints(id, [geo], constraints);
                        cx.notify();
                    });
                }
            }
            ToolId::Line | ToolId::TangentArc => {
                let Some(anchor) = interaction.anchor else {
                    interaction.anchor = Some(point);
                    interaction.anchor_ref = endpoint_ref;
                    interaction.chain_start = Some(point);
                    interaction.cursor = Some(point);
                    self.sync_sketch_gpu(cx);
                    return true;
                };
                let closes = interaction
                    .chain_start
                    .is_some_and(|start| start.distance(point) <= 1.0e-6);
                if anchor.distance(point) > 1.0e-6 {
                    let id = interaction.id;
                    let anchor_ref = interaction.anchor_ref;
                    let tangent_mid = interaction
                        .tangent_arc
                        .then_some(interaction.chain_tangent)
                        .flatten()
                        .and_then(|tangent| tangent_arc_mid(anchor, tangent, point));
                    let mut new_index = 0;
                    self.document.update(cx, |document, cx| {
                        new_index = document
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == id)
                            .map_or(0, |sketch| sketch.entities.len());
                        let mut constraints = Vec::new();
                        if let Some(existing) = anchor_ref {
                            constraints.push(Constraint::Coincident {
                                a: PointRef {
                                    entity: new_index,
                                    point: 0,
                                },
                                b: existing,
                            });
                        }
                        if let Some(existing) = endpoint_ref {
                            constraints.push(Constraint::Coincident {
                                a: PointRef {
                                    entity: new_index,
                                    point: 1,
                                },
                                b: existing,
                            });
                        }
                        if hv_snapped && tangent_mid.is_none() {
                            constraints.push(if (point.y - anchor.y).abs() < 1.0e-12 {
                                Constraint::Horizontal(EntityRef(new_index))
                            } else {
                                Constraint::Vertical(EntityRef(new_index))
                            });
                        }
                        let entity = tangent_mid.map_or(
                            SketchEntity::Line {
                                a: anchor,
                                b: point,
                            },
                            |mid| SketchEntity::Arc {
                                start: anchor,
                                end: point,
                                mid,
                            },
                        );
                        document.add_sketch_entities_with_constraints(id, [entity], constraints);
                        cx.notify();
                    });
                    interaction.anchor_ref = Some(PointRef {
                        entity: new_index,
                        point: 1,
                    });
                    interaction.chain_tangent = Some((point - anchor).normalize_or_zero());
                }
                interaction.anchor = (!closes).then_some(point);
                if closes {
                    interaction.anchor_ref = None;
                }
                interaction.cursor = (!closes).then_some(point);
                if closes {
                    interaction.chain_start = None;
                    interaction.chain_tangent = None;
                }
            }
            ToolId::Point => {
                let id = interaction.id;
                self.document.update(cx, |document, cx| {
                    document.add_sketch_items_with_constraints(
                        id,
                        [SketchItem::construction(SketchEntity::Point { at: point })],
                        [],
                    );
                    cx.notify();
                });
                interaction.cursor = None;
            }
            ToolId::Polygon | ToolId::CenterRectangle => {
                if let Some(anchor) = interaction.anchor.take() {
                    let generated = if interaction.tool == ToolId::Polygon {
                        regular_polygon(anchor, point, interaction.polygon_sides)
                    } else {
                        centered_rectangle(anchor, point)
                    };
                    if let Some((entities, constraints)) = generated {
                        let id = interaction.id;
                        self.document.update(cx, |document, cx| {
                            document.add_sketch_primitives(id, entities, constraints);
                            cx.notify();
                        });
                    }
                    interaction.cursor = None;
                } else {
                    interaction.anchor = Some(point);
                    interaction.cursor = Some(point);
                }
            }
            ToolId::Slot | ToolId::RoundedRectangle => {
                if interaction.anchor.is_none() {
                    interaction.anchor = Some(point);
                } else if interaction.arc_end.is_none() {
                    if interaction.anchor.expect("first point").distance(point) > 1.0e-6 {
                        interaction.arc_end = Some(point);
                    }
                } else {
                    let a = interaction.anchor.take().expect("first point");
                    let b = interaction.arc_end.take().expect("second point");
                    let radius = if interaction.tool == ToolId::Slot {
                        let axis = (b - a).normalize_or_zero();
                        (point - b).perp_dot(axis).abs()
                    } else {
                        point.distance(b).max(1.0e-6)
                    };
                    let generated = if interaction.tool == ToolId::Slot {
                        slot(a, b, radius)
                    } else {
                        rounded_rectangle(a, b, radius)
                    };
                    if let Some((entities, constraints)) = generated {
                        let id = interaction.id;
                        self.document.update(cx, |document, cx| {
                            document.add_sketch_primitives(id, entities, constraints);
                            cx.notify();
                        });
                    }
                }
                interaction.cursor = Some(point);
            }
            ToolId::ThreePointCircle => {
                if interaction.anchor.is_none() {
                    interaction.anchor = Some(point);
                } else if interaction.arc_end.is_none() {
                    interaction.arc_end = Some(point);
                } else {
                    let a = interaction.anchor.take().expect("first circle point");
                    let b = interaction.arc_end.take().expect("second circle point");
                    if let Some((center, radius)) = three_point_circle(a, b, point) {
                        let id = interaction.id;
                        self.document.update(cx, |document, cx| {
                            document
                                .add_sketch_entities(id, [SketchEntity::Circle { center, radius }]);
                            cx.notify();
                        });
                        self.gizmo_readout = None;
                    } else {
                        self.gizmo_readout = Some(("三点共线".into(), pointer));
                    }
                }
                interaction.cursor = Some(point);
            }
            ToolId::EllipseArc => {
                if interaction.anchor.is_none() {
                    interaction.anchor = Some(point);
                } else if interaction.arc_end.is_none() {
                    let center = interaction.anchor.expect("ellipse-arc center");
                    if center.distance(point) > 1.0e-6 {
                        interaction.arc_end = Some(point);
                    }
                } else if interaction.phase_value.is_none() {
                    let center = interaction.anchor.expect("ellipse-arc center");
                    let major = interaction.arc_end.expect("ellipse-arc major") - center;
                    let normal = glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                    interaction.phase_value = Some(
                        ((point - center).dot(normal).abs() / major.length()).clamp(1.0e-6, 1.0),
                    );
                } else if interaction.aux_point.is_none() {
                    interaction.aux_point = Some(point);
                } else {
                    let center = interaction.anchor.take().expect("ellipse-arc center");
                    let major = interaction.arc_end.take().expect("ellipse-arc major") - center;
                    let minor_ratio = interaction.phase_value.take().expect("ellipse-arc ratio");
                    let angle = |p: glam::DVec2| {
                        let x = (p - center).dot(major.normalize_or_zero()) / major.length();
                        let minor_axis = glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                        ((p - center).dot(minor_axis) / (major.length() * minor_ratio)).atan2(x)
                    };
                    let start_angle =
                        angle(interaction.aux_point.take().expect("ellipse-arc start"));
                    let mut end_angle = angle(point);
                    if end_angle <= start_angle {
                        end_angle += std::f64::consts::TAU;
                    }
                    let id = interaction.id;
                    self.document.update(cx, |document, cx| {
                        document.add_sketch_entities(
                            id,
                            [SketchEntity::EllipseArc {
                                center,
                                major,
                                minor_ratio,
                                start_angle,
                                end_angle,
                            }],
                        );
                        cx.notify();
                    });
                }
                interaction.cursor = Some(point);
            }
            ToolId::Rectangle => {
                if let Some(anchor) = interaction.anchor.take() {
                    let b = glam::DVec2::new(point.x, anchor.y);
                    let d = glam::DVec2::new(anchor.x, point.y);
                    let id = interaction.id;
                    let anchor_ref = interaction.anchor_ref.take();
                    self.document.update(cx, |document, cx| {
                        let base = document
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == id)
                            .map_or(0, |sketch| sketch.entities.len());
                        let mut constraints = vec![
                            Constraint::Coincident {
                                a: PointRef {
                                    entity: base,
                                    point: 1,
                                },
                                b: PointRef {
                                    entity: base + 1,
                                    point: 0,
                                },
                            },
                            Constraint::Coincident {
                                a: PointRef {
                                    entity: base + 1,
                                    point: 1,
                                },
                                b: PointRef {
                                    entity: base + 2,
                                    point: 0,
                                },
                            },
                            Constraint::Coincident {
                                a: PointRef {
                                    entity: base + 2,
                                    point: 1,
                                },
                                b: PointRef {
                                    entity: base + 3,
                                    point: 0,
                                },
                            },
                            Constraint::Coincident {
                                a: PointRef {
                                    entity: base + 3,
                                    point: 1,
                                },
                                b: PointRef {
                                    entity: base,
                                    point: 0,
                                },
                            },
                            Constraint::Horizontal(EntityRef(base)),
                            Constraint::Vertical(EntityRef(base + 1)),
                            Constraint::Horizontal(EntityRef(base + 2)),
                            Constraint::Vertical(EntityRef(base + 3)),
                        ];
                        if let Some(existing) = anchor_ref {
                            constraints.push(Constraint::Coincident {
                                a: PointRef {
                                    entity: base,
                                    point: 0,
                                },
                                b: existing,
                            });
                        }
                        if let Some(existing) = endpoint_ref {
                            constraints.push(Constraint::Coincident {
                                a: PointRef {
                                    entity: base + 1,
                                    point: 1,
                                },
                                b: existing,
                            });
                        }
                        document.add_sketch_entities_with_constraints(
                            id,
                            [
                                SketchEntity::Line { a: anchor, b },
                                SketchEntity::Line { a: b, b: point },
                                SketchEntity::Line { a: point, b: d },
                                SketchEntity::Line { a: d, b: anchor },
                            ],
                            constraints,
                        );
                        cx.notify();
                    });
                    interaction.cursor = None;
                } else {
                    interaction.anchor = Some(point);
                    interaction.anchor_ref = endpoint_ref;
                    interaction.cursor = Some(point);
                }
            }
            ToolId::Circle => {
                if let Some(center) = interaction.anchor.take() {
                    let radius = center.distance(point);
                    if radius > 1.0e-6 {
                        let id = interaction.id;
                        self.document.update(cx, |document, cx| {
                            document
                                .add_sketch_entities(id, [SketchEntity::Circle { center, radius }]);
                            cx.notify();
                        });
                    }
                    interaction.cursor = None;
                    self.gizmo_readout = None;
                } else {
                    interaction.anchor = Some(point);
                    interaction.cursor = Some(point);
                }
            }
            ToolId::Ellipse => {
                if interaction.anchor.is_none() {
                    interaction.anchor = Some(point);
                    interaction.cursor = Some(point);
                } else if interaction.arc_end.is_none() {
                    let center = interaction.anchor.expect("ellipse center");
                    if center.distance(point) > 1.0e-6 {
                        interaction.arc_end = Some(point);
                        interaction.cursor = Some(point);
                    }
                } else {
                    let center = interaction.anchor.take().expect("ellipse center");
                    let major = interaction.arc_end.take().expect("ellipse major") - center;
                    let perpendicular = glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                    let minor_ratio = ((point - center).dot(perpendicular).abs() / major.length())
                        .clamp(1.0e-6, 1.0);
                    let id = interaction.id;
                    self.document.update(cx, |document, cx| {
                        document.add_sketch_entities(
                            id,
                            [SketchEntity::Ellipse {
                                center,
                                major,
                                minor_ratio,
                            }],
                        );
                        cx.notify();
                    });
                    interaction.cursor = None;
                    self.gizmo_readout = None;
                }
            }
            ToolId::Arc => {
                if interaction.anchor.is_none() {
                    interaction.anchor = Some(point);
                    interaction.anchor_ref = endpoint_ref;
                    interaction.cursor = Some(point);
                } else if interaction.arc_end.is_none() {
                    interaction.arc_end = Some(point);
                    interaction.arc_end_ref = endpoint_ref;
                    interaction.cursor =
                        Some((interaction.anchor.expect("arc start") + point) * 0.5);
                } else {
                    let start = interaction.anchor.take().expect("arc start");
                    let end = interaction.arc_end.take().expect("arc end");
                    let start_ref = interaction.anchor_ref.take();
                    let end_ref = interaction.arc_end_ref.take();
                    let id = interaction.id;
                    if arc_center_radius(start, point, end).is_some() {
                        self.document.update(cx, |document, cx| {
                            let index = document
                                .sketches
                                .iter()
                                .find(|sketch| sketch.id == id)
                                .map_or(0, |sketch| sketch.entities.len());
                            let mut constraints = Vec::new();
                            if let Some(existing) = start_ref {
                                constraints.push(Constraint::Coincident {
                                    a: PointRef {
                                        entity: index,
                                        point: 0,
                                    },
                                    b: existing,
                                });
                            }
                            if let Some(existing) = end_ref {
                                constraints.push(Constraint::Coincident {
                                    a: PointRef {
                                        entity: index,
                                        point: 1,
                                    },
                                    b: existing,
                                });
                            }
                            document.add_sketch_entities_with_constraints(
                                id,
                                [SketchEntity::Arc {
                                    start,
                                    end,
                                    mid: point,
                                }],
                                constraints,
                            );
                            cx.notify();
                        });
                    }
                    interaction.cursor = None;
                    self.gizmo_readout = None;
                }
            }
            _ => return false,
        }
        interaction.hv_snapped = hv_snapped;
        self.sync_sketch_gpu(cx);
        true
    }

    fn commit_pending_spline(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(interaction) = &mut self.sketch_interaction else {
            return false;
        };
        if !matches!(interaction.tool, ToolId::Spline | ToolId::CvSpline)
            || interaction.spline_points.len()
                < if interaction.tool == ToolId::CvSpline {
                    4
                } else {
                    2
                }
        {
            return false;
        }
        let id = interaction.id;
        let points = std::mem::take(&mut interaction.spline_points);
        let start_ref = interaction.spline_start_ref.take();
        let end_ref = interaction.spline_end_ref.take();
        interaction.anchor = None;
        interaction.cursor = None;
        self.document.update(cx, |document, cx| {
            let index = document
                .sketches
                .iter()
                .find(|sketch| sketch.id == id)
                .map_or(0, |sketch| sketch.entities.len());
            let constraints =
                [(0, start_ref), (1, end_ref)]
                    .into_iter()
                    .filter_map(|(point, existing)| {
                        existing.map(|b| Constraint::Coincident {
                            a: PointRef {
                                entity: index,
                                point,
                            },
                            b,
                        })
                    });
            let geo = if interaction.tool == ToolId::CvSpline {
                SketchEntity::CvSpline {
                    control: points,
                    degree: 3,
                }
            } else {
                SketchEntity::Spline { points }
            };
            document.add_sketch_entities_with_constraints(id, [geo], constraints);
            cx.notify();
        });
        true
    }

    fn pointer(position: gpui::Point<gpui::Pixels>, scale: f32) -> Vec2 {
        Vec2::new(f32::from(position.x) * scale, f32::from(position.y) * scale)
    }

    fn changed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.dirty = true;
        // NOTE: `window.request_animation_frame()` is only legal during
        // layout/paint (it reads the currently rendering view) and aborts the
        // process when called from input handlers. `cx.notify()` schedules the
        // re-render; `render()` keeps the frame loop alive while interacting.
        cx.notify();
    }

    fn cube_rect(&self) -> orientation_cube::CubeRect {
        orientation_cube::cube_rect(
            self.rendered_size.0,
            self.rendered_size.1,
            self.device_scale,
        )
    }

    fn cube_region_at(&self, pointer: Vec2) -> Option<CubeRegion> {
        orientation_cube::pick_region(
            pointer,
            self.rendered_size.0,
            self.rendered_size.1,
            self.device_scale,
            self.camera.yaw,
            self.camera.pitch,
        )
    }

    fn go_to_cube_region(
        &mut self,
        region: CubeRegion,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let face_view = match region.signs {
            [-1, 0, 0] => Some(StandardView::Left),
            [1, 0, 0] => Some(StandardView::Right),
            [0, -1, 0] => Some(StandardView::Front),
            [0, 1, 0] => Some(StandardView::Back),
            [0, 0, -1] => Some(StandardView::Bottom),
            [0, 0, 1] => Some(StandardView::Top),
            _ => None,
        };
        if let Some(view) = face_view {
            self.go_to_standard_view(view, window, cx);
        } else {
            let (yaw, pitch) = orientation_cube::direction_to_orientation(region.direction());
            self.camera.animate_to(yaw, pitch, self.camera.distance);
            self.changed(window, cx);
        }
    }

    fn selected_body_ids(document: &Document) -> Vec<BodyId> {
        let mut ids = Vec::new();
        for item in &document.selection.items {
            if let SelItem::Body(id) = *item
                && !ids.contains(&id)
                && document
                    .bodies
                    .iter()
                    .any(|body| body.id == id && body.visible)
            {
                ids.push(id);
            }
        }
        ids
    }

    fn selection_pivot(&self, cx: &Context<Self>) -> Option<Vec3> {
        let document = self.document.read(cx);
        let ids = Self::selected_body_ids(document);
        let mut minimum = DVec3::splat(f64::INFINITY);
        let mut maximum = DVec3::splat(f64::NEG_INFINITY);
        for body in document.bodies.iter().filter(|body| ids.contains(&body.id)) {
            let (body_minimum, body_maximum) = body.shape.aabb().ok()?;
            minimum = minimum.min(body_minimum);
            maximum = maximum.max(body_maximum);
        }
        minimum
            .is_finite()
            .then_some(((minimum + maximum) * 0.5).as_vec3())
    }

    fn gizmo_scale(&self, pivot: Vec3) -> f32 {
        let distance = self.camera.eye().distance(pivot).max(0.01);
        let units_per_pixel =
            2.0 * distance * (self.camera.fov * 0.5).tan() / self.camera.viewport_size.y.max(1.0);
        // 110 logical px tall regardless of zoom or display density.
        units_per_pixel * 110.0 * self.device_scale.max(1.0)
    }

    fn gizmo_state(&self, cx: &Context<Self>) -> Option<GizmoRender> {
        let mut pivot = self
            .gizmo_drag
            .as_ref()
            .map(|drag| drag.pivot)
            .or_else(|| self.selection_pivot(cx))?;
        if let Some(GizmoDrag {
            current: Some(TransformOp::Translate(delta)),
            ..
        }) = &self.gizmo_drag
        {
            pivot += delta.as_vec3();
        }
        Some(GizmoRender {
            pivot,
            scale: self.gizmo_scale(pivot),
            hovered: self
                .gizmo_drag
                .as_ref()
                .map(|drag| drag.handle)
                .or(self.hovered_gizmo),
        })
    }

    fn selected_extrude_face(
        &self,
        cx: &Context<Self>,
    ) -> Option<(BodyId, u32, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        let [SelItem::Face(body_id, face_index)] = document.selection.items.as_slice() else {
            return None;
        };
        let body = document
            .bodies
            .iter()
            .find(|body| body.id == *body_id && body.visible && body.kind == BodyKind::Solid)?;
        let (origin, normal) = face_frame(&body.shape, *face_index)?;
        let (minimum, maximum) = body.shape.aabb().ok()?;
        let diagonal = (maximum - minimum).length().max(1.0);
        Some((*body_id, *face_index, origin, normal, diagonal))
    }

    fn selected_offset_face(&self, cx: &Context<Self>) -> Option<(BodyId, u32, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        document.selection.items.iter().find_map(|item| {
            let SelItem::Face(body_id, face_index) = *item else {
                return None;
            };
            let body = document
                .bodies
                .iter()
                .find(|body| body.id == body_id && body.visible && body.kind == BodyKind::Solid)?;
            let (origin, normal) = face_frame(&body.shape, face_index)?;
            let (minimum, maximum) = body.shape.aabb().ok()?;
            Some((
                body_id,
                face_index,
                origin,
                normal,
                (maximum - minimum).length().max(1.0),
            ))
        })
    }

    fn selected_extrude_profile(
        &self,
        cx: &Context<Self>,
    ) -> Option<(SketchId, usize, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        let [SelItem::Profile(sketch_id, profile_index)] = document.selection.items.as_slice()
        else {
            return None;
        };
        let sketch = document
            .sketches
            .iter()
            .find(|sketch| sketch.id == *sketch_id && sketch.visible)?;
        let profiles = sketch.profiles();
        let profile = profiles.get(*profile_index)?;
        let (center, diagonal) = match profile {
            Profile::Circle { center, radius } => (*center, radius * 2.0),
            Profile::Ellipse { center, major, .. } => (*center, major.length() * 2.0),
            Profile::LineLoop(points) => {
                let minimum = points
                    .iter()
                    .fold(glam::DVec2::splat(f64::INFINITY), |a, b| a.min(*b));
                let maximum = points
                    .iter()
                    .fold(glam::DVec2::splat(f64::NEG_INFINITY), |a, b| a.max(*b));
                ((minimum + maximum) * 0.5, (maximum - minimum).length())
            }
            Profile::CurveLoop(_) => (glam::DVec2::ZERO, 1.0),
        };
        Some((
            *sketch_id,
            *profile_index,
            sketch.plane.to_world(center),
            sketch.plane.normal(),
            diagonal.max(1.0),
        ))
    }

    fn selected_open_chain(
        &self,
        cx: &Context<Self>,
    ) -> Option<(SketchId, Vec<usize>, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        let mut sketch_id = None;
        let mut indices = Vec::new();
        for item in &document.selection.items {
            let SelItem::SketchEntity(id, index) = *item else {
                return None;
            };
            if sketch_id.is_some_and(|current| current != id) {
                return None;
            }
            sketch_id = Some(id);
            indices.push(index);
        }
        let sketch_id = sketch_id?;
        let sketch = document
            .sketches
            .iter()
            .find(|sketch| sketch.id == sketch_id && sketch.visible)?;
        let chain = sketch.open_chains().into_iter().find(|chain| {
            chain.len() == indices.len() && chain.iter().all(|index| indices.contains(index))
        })?;
        let shape = sketch.open_chain_wire(&chain)?.into_shape();
        let (minimum, maximum) = shape.aabb().ok()?;
        Some((
            sketch_id,
            chain,
            (minimum + maximum) * 0.5,
            sketch.plane.normal(),
            (maximum - minimum).length().max(1.0),
        ))
    }

    fn extrude_arrow_state(&self, cx: &Context<Self>) -> Option<ExtrudeArrowRender> {
        let (origin, normal) = if let Some(interaction) = &self.extrude_drag {
            (
                interaction.drag.origin,
                if interaction.opposite_phase {
                    -interaction.drag.normal
                } else {
                    interaction.drag.normal
                },
            )
        } else if let Some(interaction) = &self.profile_extrude_drag {
            (
                interaction.drag.origin,
                if interaction.opposite_phase {
                    -interaction.drag.normal
                } else {
                    interaction.drag.normal
                },
            )
        } else if let Some(interaction) = &self.open_chain_extrude_drag {
            (
                interaction.drag.origin,
                if interaction.opposite_phase {
                    -interaction.drag.normal
                } else {
                    interaction.drag.normal
                },
            )
        } else {
            (if self.active_drag_tool == Some(ToolId::OffsetFace) {
                self.selected_offset_face(cx)
            } else {
                self.selected_extrude_face(cx)
            })
            .map(|(_, _, origin, normal, _)| (origin, normal))
            .or_else(|| {
                (self.active_drag_tool != Some(ToolId::OffsetFace))
                    .then(|| self.selected_extrude_profile(cx))
                    .flatten()
                    .map(|(_, _, origin, normal, _)| (origin, normal))
            })
            .or_else(|| {
                (self.active_drag_tool != Some(ToolId::OffsetFace))
                    .then(|| self.selected_open_chain(cx))
                    .flatten()
                    .map(|(_, _, origin, normal, _)| (origin, normal))
            })?
        };
        let origin = origin.as_vec3();
        Some(ExtrudeArrowRender {
            origin,
            normal: normal.as_vec3(),
            scale: self.gizmo_scale(origin),
            hovered: self.hovered_extrude_arrow
                || self.extrude_drag.is_some()
                || self.profile_extrude_drag.is_some()
                || self.open_chain_extrude_drag.is_some(),
        })
    }

    fn selected_dressup(
        &self,
        cx: &Context<Self>,
    ) -> Option<(BodyId, Vec<u32>, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        let body_id = document
            .selection
            .items
            .iter()
            .find_map(|item| match item {
                SelItem::Edge(id, _) => Some(*id),
                _ => None,
            })?;
        let edge_indices: Vec<_> = document
            .selection
            .items
            .iter()
            .filter_map(|item| match item {
                SelItem::Edge(id, index) if *id == body_id => Some(*index),
                _ => None,
            })
            .collect();
        let body = document
            .bodies
            .iter()
            .find(|body| body.id == body_id && body.visible && body.kind == BodyKind::Solid)?;
        let (origin, direction) = edge_frame(&body.shape, edge_indices[0])?;
        let (minimum, maximum) = body.shape.aabb().ok()?;
        Some((
            body_id,
            edge_indices,
            origin,
            direction,
            (maximum - minimum).length().max(1.0),
        ))
    }

    fn selected_shell(&self, cx: &Context<Self>) -> Option<(BodyId, Vec<u32>, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        let body_id = document
            .selection
            .items
            .iter()
            .find_map(|item| match item {
                SelItem::Face(id, _) => Some(*id),
                _ => None,
            })?;
        let face_indices: Vec<_> = document
            .selection
            .items
            .iter()
            .filter_map(|item| match item {
                SelItem::Face(id, index) if *id == body_id => Some(*index),
                _ => None,
            })
            .collect();
        let body = document
            .bodies
            .iter()
            .find(|body| body.id == body_id && body.visible && body.kind == BodyKind::Solid)?;
        let (origin, outward) = face_frame(&body.shape, face_indices[0])?;
        let (minimum, maximum) = body.shape.aabb().ok()?;
        Some((
            body_id,
            face_indices,
            origin,
            -outward,
            (maximum - minimum).length().max(1.0),
        ))
    }

    fn selected_thicken(&self, cx: &Context<Self>) -> Option<(BodyId, DVec3, DVec3, f64)> {
        let document = self.document.read(cx);
        let [SelItem::Body(body_id)] = document.selection.items.as_slice() else {
            return None;
        };
        let body = document
            .bodies
            .iter()
            .find(|body| body.id == *body_id && body.visible && body.kind == BodyKind::Surface)?;
        let (minimum, maximum) = body.shape.aabb().ok()?;
        let origin = (minimum + maximum) * 0.5;
        let center = body.shape.face_center_of_mass(0).ok()?;
        let mut direction = body
            .shape
            .face_normal_at(0, center)
            .ok()?
            .normalize_or_zero();
        if body.shape.face_is_reversed(0).ok()? {
            direction = -direction;
        }
        if direction.length_squared() < 0.99 {
            return None;
        }
        Some((
            *body_id,
            origin,
            direction,
            (maximum - minimum).length().max(1.0),
        ))
    }

    fn tool_arrow_state(&self, cx: &Context<Self>) -> Option<ExtrudeArrowRender> {
        let (origin, direction) =
            if let Some(M6Interaction::ConstructionPlane { base, distance, .. }) =
                &self.m6_interaction
            {
                (base.origin + base.normal() * *distance, base.normal())
            } else if let Some(interaction) = &self.dressup_drag {
                (interaction.drag.origin, interaction.drag.direction)
            } else if let Some(interaction) = &self.shell_drag {
                (interaction.drag.origin, interaction.drag.direction)
            } else if let Some(interaction) = &self.thicken_drag {
                (interaction.origin, interaction.direction)
            } else {
                match self.active_drag_tool {
                    Some(ToolId::Fillet | ToolId::Chamfer) => {
                        let (_, _, origin, direction, _) = self.selected_dressup(cx)?;
                        (origin, direction)
                    }
                    Some(ToolId::Shell) => {
                        let (_, _, origin, direction, _) = self.selected_shell(cx)?;
                        (origin, direction)
                    }
                    Some(ToolId::Thicken) => {
                        let (_, origin, direction, _) = self.selected_thicken(cx)?;
                        (origin, direction)
                    }
                    _ => return self.extrude_arrow_state(cx),
                }
            };
        let origin = origin.as_vec3();
        Some(ExtrudeArrowRender {
            origin,
            normal: direction.as_vec3(),
            scale: self.gizmo_scale(origin),
            hovered: self.hovered_extrude_arrow
                || self.dressup_drag.is_some()
                || self.shell_drag.is_some()
                || self.thicken_drag.is_some(),
        })
    }

    /// Arms a fillet, chamfer, shell, or Offset Face arrow for the selection.
    pub fn activate_drag_tool(
        &mut self,
        tool: ToolId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.finish_dressup_drag(false, cx);
        self.finish_shell_drag(false, cx);
        self.finish_thicken_drag(false, cx);
        let applicable = match tool {
            ToolId::Fillet | ToolId::Chamfer => self.selected_dressup(cx).is_some(),
            ToolId::Shell => self.selected_shell(cx).is_some(),
            ToolId::Thicken => self.selected_thicken(cx).is_some(),
            ToolId::OffsetFace => self.selected_offset_face(cx).is_some(),
            _ => true,
        };
        if !applicable {
            self.active_drag_tool = None;
            self.gizmo_readout = Some((
                "所选几何不支持此操作；实体专用工具不能用于曲面体".to_owned(),
                self.last_pointer + Vec2::new(14.0, -22.0),
            ));
            self.changed(window, cx);
            return;
        }
        self.active_drag_tool = Some(tool);
        self.gizmo_readout = match tool {
            ToolId::Shell => self.selected_shell(cx).map(|(_, _, origin, _, _)| {
                (
                    "t 2.0".to_string(),
                    self.camera.project(origin.as_vec3()) + Vec2::new(14.0, -22.0),
                )
            }),
            ToolId::Thicken => self.selected_thicken(cx).map(|(_, origin, _, _)| {
                (
                    "t 2.0".to_string(),
                    self.camera.project(origin.as_vec3()) + Vec2::new(14.0, -22.0),
                )
            }),
            _ => None,
        };
        self.changed(window, cx);
    }

    /// Shows a short modeling-operation rejection beside the pointer.
    pub fn show_modeling_hint(
        &mut self,
        message: impl Into<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.gizmo_readout = Some((message.into(), self.last_pointer + Vec2::new(14.0, -22.0)));
        self.changed(window, cx);
    }

    /// Activates an M6 transform or construction-plane interaction.
    pub fn activate_m6_tool(&mut self, tool: ToolId, window: &mut Window, cx: &mut Context<Self>) {
        let document = self.document.read(cx);
        let mut ids: Vec<_> = document
            .selection
            .items
            .iter()
            .filter_map(|item| match item {
                SelItem::Body(id) => Some(*id),
                _ => None,
            })
            .collect();
        ids.dedup();
        self.m6_interaction = match tool {
            ToolId::Plane => {
                let base = document
                    .selection
                    .items
                    .iter()
                    .find_map(|item| match *item {
                        SelItem::Face(body, face) => document
                            .bodies
                            .iter()
                            .find(|item| item.id == body)
                            .and_then(|item| SketchPlane::from_face(&item.shape, face)),
                        _ => None,
                    })
                    .unwrap_or_else(SketchPlane::xy);
                Some(M6Interaction::ConstructionPlane {
                    base,
                    distance: 0.0,
                    drag: None,
                })
            }
            ToolId::Scale if !ids.is_empty() => {
                let (minimum, maximum) = ids
                    .iter()
                    .filter_map(|id| document.bodies.iter().find(|body| body.id == *id))
                    .filter_map(|body| body.shape.aabb().ok())
                    .fold(
                        (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
                        |(min, max), (body_min, body_max)| (min.min(body_min), max.max(body_max)),
                    );
                Some(M6Interaction::Scale {
                    ids,
                    pivot: (minimum + maximum) * 0.5,
                    factor: 1.0,
                    drag: None,
                })
            }
            ToolId::Split if ids.len() == 1 => {
                let body = document
                    .bodies
                    .iter()
                    .find(|body| body.id == ids[0])
                    .expect("selected body");
                body.shape
                    .aabb()
                    .ok()
                    .map(|(minimum, maximum)| M6Interaction::Split {
                        id: ids[0],
                        y: (minimum.y + maximum.y) * 0.5,
                        drag: None,
                    })
            }
            ToolId::Align if ids.len() >= 2 => Some(M6Interaction::Align {
                ids,
                axes: [false, true, true],
            }),
            _ => None,
        };
        if let Some(M6Interaction::Split { y, .. }) = self.m6_interaction {
            self.section_enabled = true;
            self.section_offset = Some(y as f32);
        }
        self.gizmo_readout = match &self.m6_interaction {
            Some(M6Interaction::Scale { .. }) => Some((
                "×1.00".to_owned(),
                self.last_pointer + Vec2::new(14.0, -22.0),
            )),
            Some(M6Interaction::Split { y, .. }) => Some((
                format!("Y {y:.1} · Enter"),
                self.last_pointer + Vec2::new(14.0, -22.0),
            )),
            Some(M6Interaction::Align { .. }) => Some((
                "[X] [Y✓] [Z✓] · Enter".to_owned(),
                self.last_pointer + Vec2::new(14.0, -22.0),
            )),
            Some(M6Interaction::ConstructionPlane { distance, .. }) => Some((
                format!("offset {distance:.1} · Enter"),
                self.last_pointer + Vec2::new(14.0, -22.0),
            )),
            None => None,
        };
        self.changed(window, cx);
    }

    fn commit_m6(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(interaction) = self.m6_interaction.take() else {
            return false;
        };
        if matches!(interaction, M6Interaction::Split { .. }) {
            self.section_enabled = false;
        }
        self.document.update(cx, |document, cx| {
            match interaction {
                M6Interaction::Scale {
                    ids, pivot, factor, ..
                } => {
                    document.apply_scale(&ids, factor, pivot);
                }
                M6Interaction::Split { id, y, .. } => {
                    document.apply_split(id, y);
                }
                M6Interaction::Align { ids, axes } => {
                    document.apply_align(&ids, axes);
                }
                M6Interaction::ConstructionPlane { base, distance, .. } => {
                    document.add_offset_construction_plane(base, distance);
                }
            }
            cx.notify();
        });
        self.gizmo_readout = None;
        true
    }

    fn toggle_align_axis(&mut self, axis: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(M6Interaction::Align { axes, .. }) = &mut self.m6_interaction {
            axes[axis] = !axes[axis];
            self.changed(window, cx);
        }
    }

    fn confirm_m6(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_m6(cx);
        self.changed(window, cx);
    }

    fn axis_from_item(&self, item: SelItem, cx: &Context<Self>) -> Option<AxisReference> {
        match item {
            SelItem::Axis(id) => {
                let document = self.document.read(cx);
                let axis = document
                    .construction_axes
                    .iter()
                    .find(|axis| axis.id == id)?;
                Some(AxisReference {
                    origin: axis.origin,
                    direction: axis.direction,
                    label: "construction axis",
                })
            }
            SelItem::Point(id) => {
                let document = self.document.read(cx);
                let point = document
                    .construction_points
                    .iter()
                    .find(|point| point.id == id)?;
                Some(AxisReference {
                    origin: point.position,
                    direction: DVec3::Z,
                    label: "construction point / Z",
                })
            }
            SelItem::Edge(body, edge) => {
                let edge = self
                    .scene_meshes
                    .iter()
                    .find(|mesh| mesh.id == body)?
                    .mesh
                    .edges
                    .get(edge as usize)?;
                let (origin, direction) = fit_straight_edge(&edge.points)?;
                Some(AxisReference {
                    origin,
                    direction,
                    label: "edge",
                })
            }
            SelItem::SketchEntity(sketch_id, entity) => {
                let document = self.document.read(cx);
                let sketch = document
                    .sketches
                    .iter()
                    .find(|sketch| sketch.id == sketch_id)?;
                let SketchEntity::Line { a, b } = &sketch.entities.get(entity)?.geo else {
                    return None;
                };
                let origin = sketch.plane.to_world(*a);
                let direction = sketch.plane.to_world(*b) - origin;
                (direction.length_squared() > 1.0e-12).then_some(AxisReference {
                    origin,
                    direction: direction.normalize(),
                    label: "sketch line",
                })
            }
            _ => None,
        }
    }

    fn plane_from_item(&self, item: SelItem, cx: &Context<Self>) -> Option<PlaneReference> {
        match item {
            SelItem::Point(id) => {
                let document = self.document.read(cx);
                let point = document
                    .construction_points
                    .iter()
                    .find(|point| point.id == id)?;
                Some(PlaneReference {
                    origin: point.position,
                    normal: DVec3::Z,
                    label: "construction point / XY",
                })
            }
            SelItem::Plane(id) => {
                let plane = self
                    .document
                    .read(cx)
                    .construction_planes
                    .iter()
                    .find(|plane| plane.id == id)?
                    .plane;
                Some(PlaneReference {
                    origin: plane.origin,
                    normal: plane.normal(),
                    label: "construction plane",
                })
            }
            SelItem::Face(body_id, face_index) => {
                let document = self.document.read(cx);
                let body = document.bodies.iter().find(|body| body.id == body_id)?;
                let plane = SketchPlane::from_face(&body.shape, face_index)?;
                Some(PlaneReference {
                    origin: plane.origin,
                    normal: plane.normal(),
                    label: "face",
                })
            }
            SelItem::Profile(sketch_id, _) | SelItem::SketchEntity(sketch_id, _) => {
                let document = self.document.read(cx);
                let plane = document
                    .sketches
                    .iter()
                    .find(|sketch| sketch.id == sketch_id)?
                    .plane;
                Some(PlaneReference {
                    origin: plane.origin,
                    normal: plane.normal(),
                    label: "sketch plane",
                })
            }
            _ => None,
        }
    }

    fn sketch_reference_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<SelItem> {
        let document = self.document.read(cx);
        let threshold = 6.0 * self.device_scale.max(1.0);
        for sketch in document
            .sketches
            .iter()
            .rev()
            .filter(|sketch| sketch.visible)
        {
            if let Some((entity, _)) = sketch
                .entities
                .iter()
                .enumerate()
                .filter_map(|(index, entity)| {
                    let distance = match &entity.geo {
                        SketchEntity::Line { a, b } => point_segment_distance(
                            pointer,
                            self.camera.project(sketch.plane.to_world(*a).as_vec3()),
                            self.camera.project(sketch.plane.to_world(*b).as_vec3()),
                        ),
                        SketchEntity::Circle { center, radius } => (0..64)
                            .map(|index| {
                                let angle =
                                    |index: usize| index as f64 / 64.0 * std::f64::consts::TAU;
                                let point = |angle: f64| {
                                    self.camera.project(
                                        sketch
                                            .plane
                                            .to_world(
                                                *center
                                                    + glam::DVec2::new(angle.cos(), angle.sin())
                                                        * *radius,
                                            )
                                            .as_vec3(),
                                    )
                                };
                                point_segment_distance(
                                    pointer,
                                    point(angle(index)),
                                    point(angle(index + 1)),
                                )
                            })
                            .fold(f32::INFINITY, f32::min),
                        SketchEntity::Ellipse {
                            center,
                            major,
                            minor_ratio,
                        } => sample_ellipse(*center, *major, *minor_ratio, 64)
                            .windows(2)
                            .map(|points| {
                                point_segment_distance(
                                    pointer,
                                    self.camera
                                        .project(sketch.plane.to_world(points[0]).as_vec3()),
                                    self.camera
                                        .project(sketch.plane.to_world(points[1]).as_vec3()),
                                )
                            })
                            .fold(f32::INFINITY, f32::min),
                        SketchEntity::Arc { start, mid, end } => sample_arc(*start, *mid, *end, 32)
                            .windows(2)
                            .map(|points| {
                                point_segment_distance(
                                    pointer,
                                    self.camera
                                        .project(sketch.plane.to_world(points[0]).as_vec3()),
                                    self.camera
                                        .project(sketch.plane.to_world(points[1]).as_vec3()),
                                )
                            })
                            .fold(f32::INFINITY, f32::min),
                        SketchEntity::Spline { points } => sketch
                            .spline_polyline(index, points)
                            .windows(2)
                            .map(|points| {
                                point_segment_distance(
                                    pointer,
                                    self.camera
                                        .project(sketch.plane.to_world(points[0]).as_vec3()),
                                    self.camera
                                        .project(sketch.plane.to_world(points[1]).as_vec3()),
                                )
                            })
                            .fold(f32::INFINITY, f32::min),
                        SketchEntity::CvSpline { control, degree } => {
                            crate::sketch::sample_cv_spline(control, *degree, sketch.plane)
                                .windows(2)
                                .map(|points| {
                                    point_segment_distance(
                                        pointer,
                                        self.camera
                                            .project(sketch.plane.to_world(points[0]).as_vec3()),
                                        self.camera
                                            .project(sketch.plane.to_world(points[1]).as_vec3()),
                                    )
                                })
                                .fold(f32::INFINITY, f32::min)
                        }
                        SketchEntity::EllipseArc {
                            center,
                            major,
                            minor_ratio,
                            start_angle,
                            end_angle,
                        } => sample_ellipse_arc(
                            *center,
                            *major,
                            *minor_ratio,
                            *start_angle,
                            *end_angle,
                            32,
                        )
                        .windows(2)
                        .map(|points| {
                            point_segment_distance(
                                pointer,
                                self.camera
                                    .project(sketch.plane.to_world(points[0]).as_vec3()),
                                self.camera
                                    .project(sketch.plane.to_world(points[1]).as_vec3()),
                            )
                        })
                        .fold(f32::INFINITY, f32::min),
                        SketchEntity::Point { at } => pointer
                            .distance(self.camera.project(sketch.plane.to_world(*at).as_vec3())),
                    };
                    (distance <= threshold).then_some((index, distance))
                })
                .min_by(|left, right| left.1.total_cmp(&right.1))
            {
                return Some(SelItem::SketchEntity(sketch.id, entity));
            }

            let (ray_origin, ray) = self.camera.unproject_ray(pointer);
            let normal = sketch.plane.normal().as_vec3();
            let denominator = ray.dot(normal);
            if denominator.abs() > 1.0e-6 {
                let distance =
                    (sketch.plane.origin.as_vec3() - ray_origin).dot(normal) / denominator;
                if distance >= 0.0 {
                    let local = sketch
                        .plane
                        .to_local((ray_origin + ray * distance).as_dvec3());
                    if let Some((index, _)) = sketch
                        .profiles()
                        .iter()
                        .enumerate()
                        .find(|(_, profile)| profile_contains(sketch, profile, local))
                    {
                        return Some(SelItem::Profile(sketch.id, index));
                    }
                }
            }
        }
        None
    }

    fn axis_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<AxisReference> {
        if let Some(item @ (SelItem::Axis(_) | SelItem::Point(_))) =
            self.construction_plane_at(pointer, cx)
        {
            return self.axis_from_item(item, cx);
        }
        if let Some(item) = self.sketch_reference_at(pointer, cx)
            && let Some(axis) = self.axis_from_item(item, cx)
        {
            return Some(axis);
        }
        let hit = pick_edge(&self.pick_bodies(), &self.camera, pointer, 6.0)?;
        self.axis_from_item(SelItem::Edge(hit.body, hit.edge), cx)
    }

    fn plane_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<PlaneReference> {
        if let Some(item @ (SelItem::Plane(_) | SelItem::Point(_))) =
            self.construction_plane_at(pointer, cx)
        {
            return self.plane_from_item(item, cx);
        }
        if let Some(item) = self.sketch_reference_at(pointer, cx) {
            return self.plane_from_item(item, cx);
        }
        let (origin, ray) = self.camera.unproject_ray(pointer);
        let hit = pick_face(&self.pick_bodies(), origin, ray)?;
        self.plane_from_item(SelItem::Face(hit.body, hit.face), cx)
    }

    fn pending_reference_kind(&self) -> Option<ReferenceKind> {
        match self.pending_reference.as_ref()? {
            PendingReference::Revolve { .. }
            | PendingReference::Pattern
            | PendingReference::Helix
            | PendingReference::SketchMirror { .. } => Some(ReferenceKind::Axis),
            PendingReference::Mirror { .. }
            | PendingReference::Draft { .. }
            | PendingReference::ReplaceFace { .. } => Some(ReferenceKind::Plane),
        }
    }

    fn show_reference_hint(&mut self) {
        let Some(kind) = self.pending_reference_kind() else {
            return;
        };
        self.gizmo_readout = Some((
            match kind {
                ReferenceKind::Axis => {
                    "Pick axis: click an edge or sketch line · or press Enter for default"
                }
                ReferenceKind::Plane => {
                    "Pick plane: click a planar face or sketch · or press Enter for default"
                }
            }
            .to_owned(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
    }

    /// Starts the two-pick connector-frame workflow for a revolute joint.
    pub fn begin_joint_tool(&mut self, cx: &mut Context<Self>) {
        self.joint_tool_active = true;
        self.joint_first = None;
        self.gizmo_readout = Some((
            "拾取第一个连接框架 · 平面/圆边/构造轴或点".to_owned(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
        cx.notify();
    }

    /// Enables geometry dragging as a one-DOF assembly drive gesture.
    pub fn set_joint_drive_enabled(&mut self, enabled: bool) {
        self.joint_drive_enabled = enabled;
        if !enabled {
            self.joint_drive = None;
        }
    }

    fn begin_joint_drive(&mut self, pointer: Vec2, cx: &Context<Self>) -> bool {
        if !self.joint_drive_enabled {
            return false;
        }
        let Some(body) = self
            .item_at(pointer, true, SelectionFilter::Body)
            .and_then(SelItem::body_id)
        else {
            return false;
        };
        let document = self.document.read(cx);
        let driving: Vec<_> = document
            .joints
            .iter()
            .filter(|joint| {
                (joint.a.0 == body || joint.b.0 == body)
                    && matches!(
                        joint.kind,
                        JointKind::Revolute | JointKind::Slider | JointKind::Cylindrical
                    )
            })
            .collect();
        let [joint] = driving.as_slice() else {
            return false;
        };
        self.joint_drive = Some(JointDriveInteraction {
            id: joint.id,
            kind: joint.kind,
            start_x: pointer.x,
            start_value: joint.value,
            start_value2: joint.value2,
            current_value: joint.value,
        });
        true
    }

    fn joint_connector_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<(BodyId, Connector)> {
        let item = self
            .construction_plane_at(pointer, cx)
            .or_else(|| self.item_at(pointer, false, SelectionFilter::Auto))?;
        let document = self.document.read(cx);
        let selected_body = document
            .selection
            .items
            .iter()
            .find_map(|item| item.body_id());
        let (body, source) = match item {
            SelItem::Face(body, index) => {
                let shape = &document.bodies.iter().find(|item| item.id == body)?.shape;
                (body, ConnectorSource::Face(face_ref(shape, index)))
            }
            SelItem::Edge(body, index) => {
                let shape = &document.bodies.iter().find(|item| item.id == body)?.shape;
                (body, ConnectorSource::Edge(edge_ref(shape, index)))
            }
            SelItem::Plane(id) => (selected_body?, ConnectorSource::Plane(id)),
            SelItem::Axis(id) => (selected_body?, ConnectorSource::Axis(id)),
            SelItem::Point(id) => (selected_body?, ConnectorSource::Point(id)),
            _ => return None,
        };
        let mut connector = Connector {
            frame: ConnectorFrame {
                origin: DVec3::ZERO,
                z: DVec3::Z,
                x: DVec3::X,
            },
            source,
            stale: false,
        };
        document.refresh_connector(body, &mut connector);
        (!connector.stale).then_some((body, connector))
    }

    fn pick_joint_connector(&mut self, pointer: Vec2, cx: &mut Context<Self>) -> bool {
        let Some(candidate) = self.joint_connector_at(pointer, cx) else {
            self.gizmo_readout = Some((
                "请选择平面、圆边、构造轴或点".to_owned(),
                pointer + Vec2::new(14.0, -22.0),
            ));
            return true;
        };
        if let Some(first) = self.joint_first.take() {
            if first.0 == candidate.0 {
                self.joint_first = Some(first);
                self.gizmo_readout = Some((
                    "第二个连接框架必须属于不同实体".to_owned(),
                    pointer + Vec2::new(14.0, -22.0),
                ));
                return true;
            }
            self.document.update(cx, |document, cx| {
                let number = document.joints.len() + 1;
                document.add_joint(Joint {
                    id: JointId(0),
                    name: format!("Joint {number}"),
                    kind: JointKind::Revolute,
                    a: first,
                    b: candidate,
                    value: 0.0,
                    value2: 0.0,
                    limits: None,
                });
                cx.notify();
            });
            self.joint_tool_active = false;
            self.gizmo_readout = Some((
                "关节已创建 · 旋转".to_owned(),
                pointer + Vec2::new(14.0, -22.0),
            ));
        } else {
            self.joint_first = Some(candidate);
            self.gizmo_readout = Some((
                "拾取第二个连接框架 · 必须属于不同实体".to_owned(),
                pointer + Vec2::new(14.0, -22.0),
            ));
        }
        true
    }

    fn resolve_reference(&mut self, geometry: ReferenceGeometry, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_reference.take() else {
            return false;
        };
        match (pending, geometry) {
            (PendingReference::Revolve { source, .. }, ReferenceGeometry::Axis(axis)) => {
                self.revolve_interaction = Some(RevolveInteraction {
                    source,
                    axis,
                    angle_degrees: 360.0,
                    start_x: None,
                    moved: false,
                    mode: self.extrude_mode,
                    expression: None,
                });
                self.gizmo_readout = Some((
                    format!("axis: {} · 360°", axis.label),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            }
            (PendingReference::Mirror { ids, .. }, ReferenceGeometry::Plane(plane)) => {
                self.document.update(cx, |document, cx| {
                    if !document
                        .apply_mirror(&ids, plane.origin, plane.normal)
                        .is_empty()
                    {
                        cx.notify();
                    }
                });
                self.gizmo_readout = Some((
                    format!("plane: {}", plane.label),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            }
            (PendingReference::Draft { body, faces, .. }, ReferenceGeometry::Plane(plane)) => {
                self.draft_interaction = Some(DraftInteraction {
                    body,
                    faces,
                    neutral: plane,
                    angle_degrees: 5.0,
                    start_x: None,
                    expression: None,
                });
                self.gizmo_readout = Some((
                    format!("neutral: {} · 5.0°", plane.label),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            }
            (PendingReference::Pattern, ReferenceGeometry::Axis(axis)) => {
                if let Some(interaction) = &mut self.pattern_interaction {
                    interaction.axis = Some(axis);
                    self.gizmo_readout = Some((
                        match interaction.mode {
                            PatternMode::Linear => {
                                format!("axis: {} · {:.1}", axis.label, interaction.spacing)
                            }
                            PatternMode::Circular => {
                                format!("axis: {} · 360° / {}", axis.label, interaction.count)
                            }
                        },
                        self.last_pointer + Vec2::new(14.0, -22.0),
                    ));
                }
            }
            (
                PendingReference::ReplaceFace { body, face_index },
                ReferenceGeometry::Plane(plane),
            ) => {
                self.document.update(cx, |document, cx| {
                    if document.apply_replace_face(body, face_index, plane.origin, plane.normal) {
                        cx.notify();
                    }
                });
                self.gizmo_readout = Some((
                    format!("目标面：{}", plane.label),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            }
            (PendingReference::Helix, ReferenceGeometry::Axis(axis)) => {
                self.helix_interaction = Some(HelixInteraction {
                    axis,
                    radius: 20.0,
                    pitch: 10.0,
                    turns: 5.0,
                    profile_radius: 1.0,
                    left_handed: false,
                    phase: HelixPhase::Radius,
                    expressions: [None, None, None, None],
                });
                self.gizmo_readout = Some((
                    format!("轴：{} · 输入半径", axis.label),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            }
            (pending @ PendingReference::SketchMirror { .. }, _) => {
                self.pending_reference = Some(pending);
                return false;
            }
            (pending, _) => {
                self.pending_reference = Some(pending);
                return false;
            }
        }
        true
    }

    fn pick_pending_reference(&mut self, pointer: Vec2, cx: &mut Context<Self>) -> bool {
        if let Some(PendingReference::SketchMirror { id, entities }) = self.pending_reference.take()
        {
            let axis = match self.sketch_entity_at(pointer, cx) {
                Some(SelItem::SketchEntity(axis_id, axis))
                    if axis_id == id
                        && self
                            .document
                            .read(cx)
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == id)
                            .and_then(|sketch| sketch.entities.get(axis))
                            .is_some_and(|item| matches!(item.geo, SketchEntity::Line { .. })) =>
                {
                    Some(axis)
                }
                _ => None,
            };
            if let Some(axis) = axis {
                self.document.update(cx, |document, cx| {
                    if document.mirror_sketch_entities(id, &entities, axis) {
                        cx.notify();
                    }
                });
                self.gizmo_readout = Some((
                    "草图镜像已创建".into(),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
                self.sync_sketch_gpu(cx);
            } else {
                self.pending_reference = Some(PendingReference::SketchMirror { id, entities });
                self.show_reference_hint();
            }
            return true;
        }
        let geometry = match self.pending_reference_kind() {
            Some(ReferenceKind::Axis) => self.axis_at(pointer, cx).map(ReferenceGeometry::Axis),
            Some(ReferenceKind::Plane) => self.plane_at(pointer, cx).map(ReferenceGeometry::Plane),
            None => return false,
        };
        if let Some(geometry) = geometry {
            self.resolve_reference(geometry, cx);
        } else {
            self.show_reference_hint();
        }
        true
    }

    fn accept_default_reference(&mut self, cx: &mut Context<Self>) -> bool {
        let geometry = match self.pending_reference.as_ref() {
            Some(PendingReference::Revolve { default, .. }) => ReferenceGeometry::Axis(*default),
            Some(PendingReference::Mirror { default, .. }) => ReferenceGeometry::Plane(*default),
            Some(PendingReference::Draft { default, .. }) => ReferenceGeometry::Plane(*default),
            Some(PendingReference::Pattern) => {
                let mode = self
                    .pattern_interaction
                    .as_ref()
                    .map(|interaction| interaction.mode)
                    .unwrap_or(PatternMode::Linear);
                ReferenceGeometry::Axis(AxisReference {
                    origin: DVec3::ZERO,
                    direction: if mode == PatternMode::Linear {
                        DVec3::X
                    } else {
                        DVec3::Z
                    },
                    label: if mode == PatternMode::Linear {
                        "world X default"
                    } else {
                        "world Z default"
                    },
                })
            }
            Some(PendingReference::Helix) => ReferenceGeometry::Axis(AxisReference {
                origin: DVec3::ZERO,
                direction: DVec3::Z,
                label: "world Z default",
            }),
            Some(PendingReference::ReplaceFace { .. }) => return false,
            Some(PendingReference::SketchMirror { .. }) => return false,
            None => return false,
        };
        self.resolve_reference(geometry, cx)
    }

    /// Arms horizontal-drag revolve for the selected profile.
    pub fn activate_revolve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected_open = self
            .selected_open_chain(cx)
            .map(|(sketch, entity_indices, _, _, _)| (sketch, entity_indices));
        let selected = {
            let document = self.document.read(cx);
            if let Some((sketch_id, entity_indices)) = selected_open {
                document
                    .sketches
                    .iter()
                    .find(|sketch| sketch.id == sketch_id)
                    .map(|sketch| {
                        (
                            RevolveSource::OpenChain {
                                sketch: sketch_id,
                                entity_indices,
                            },
                            sketch.plane,
                        )
                    })
            } else {
                document
                    .selection
                    .items
                    .iter()
                    .find_map(|item| match *item {
                        source @ SelItem::Profile(sketch_id, _) => document
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == sketch_id)
                            .map(|sketch| (RevolveSource::Profile(source), sketch.plane)),
                        _ => None,
                    })
            }
        };
        let Some((source, plane)) = selected else {
            return;
        };
        let inferred = self
            .document
            .read(cx)
            .selection
            .items
            .iter()
            .copied()
            .find_map(|item| self.axis_from_item(item, cx));
        self.cancel_feature_interaction();
        self.pending_reference = Some(PendingReference::Revolve {
            source,
            default: AxisReference {
                origin: plane.origin,
                direction: plane.y_axis,
                label: "sketch Y default",
            },
        });
        if let Some(axis) = inferred {
            self.resolve_reference(ReferenceGeometry::Axis(axis), cx);
        } else {
            self.show_reference_hint();
        }
        self.changed(window, cx);
    }

    /// Arms Replace Face after validating one selected planar source face.
    pub fn activate_replace_face(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let source = {
            let document = self.document.read(cx);
            document
                .selection
                .items
                .iter()
                .find_map(|item| match *item {
                    SelItem::Face(body, face_index) => document
                        .bodies
                        .iter()
                        .find(|candidate| candidate.id == body)
                        .and_then(|candidate| {
                            SketchPlane::from_face(&candidate.shape, face_index)
                                .map(|_| (body, face_index))
                        }),
                    _ => None,
                })
        };
        let Some((body, face_index)) = source else {
            return;
        };
        self.cancel_feature_interaction();
        self.pending_reference = Some(PendingReference::ReplaceFace { body, face_index });
        self.gizmo_readout = Some((
            "点击目标面或构造平面".to_owned(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
        self.changed(window, cx);
    }

    /// Arms projection of the next picked body edge or face into the active sketch.
    pub fn activate_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.document.read(cx).active_sketch.is_none() {
            return;
        }
        self.project_pending = true;
        self.gizmo_readout = Some((
            "点击要投影的边或面轮廓".to_owned(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
        self.changed(window, cx);
    }

    /// Arms helix parameters after an optional picked axis (Enter uses world Z).
    pub fn activate_helix(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_feature_interaction();
        self.pending_reference = Some(PendingReference::Helix);
        self.show_reference_hint();
        self.changed(window, cx);
    }

    /// Arms thread type/mode badges and two numeric phases (pitch, depth).
    pub fn activate_thread(
        &mut self,
        body: BodyId,
        face: u32,
        depth: f64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_feature_interaction();
        let Some((origin, axis, radius, _)) = self
            .document
            .read(cx)
            .bodies
            .iter()
            .find(|candidate| candidate.id == body)
            .and_then(|body| body.shape.face_cylinder_data(face as usize).ok())
        else {
            return;
        };
        self.thread_interaction = Some(ThreadInteraction {
            body,
            face,
            external: true,
            mode: crate::document::ThreadMode::Cosmetic,
            pitch: 2.0,
            depth,
            phase: ThreadPhase::Pitch,
            origin,
            axis,
            radius,
        });
        self.gizmo_readout = Some((
            "输入螺距".into(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
        self.sync_sketch_gpu(cx);
        self.changed(window, cx);
    }

    fn set_thread_external(&mut self, external: bool, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(interaction) = &mut self.thread_interaction {
            interaction.external = external;
        }
        self.sync_sketch_gpu(cx);
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn set_thread_mode(
        &mut self,
        mode: crate::document::ThreadMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(interaction) = &mut self.thread_interaction {
            interaction.mode = mode;
        }
        self.sync_sketch_gpu(cx);
        cx.stop_propagation();
        self.changed(window, cx);
    }

    /// Arms planar-face point picking followed by a diameter drag.
    pub fn activate_hole(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = {
            let document = self.document.read(cx);
            document
                .selection
                .items
                .iter()
                .find_map(|item| match *item {
                    SelItem::Face(body, face_index) => document
                        .bodies
                        .iter()
                        .find(|candidate| candidate.id == body)
                        .and_then(|candidate| {
                            SketchPlane::from_face(&candidate.shape, face_index)
                                .map(|plane| (body, face_index, plane))
                        }),
                    _ => None,
                })
        };
        let Some((body, face_index, plane)) = selected else {
            return;
        };
        self.cancel_feature_interaction();
        self.hole_interaction = Some(HoleInteraction {
            body,
            face_index,
            plane,
            at: None,
            diameter: 10.0,
            kind: HoleKind::Through,
            cut: HoleCut::None,
            start_x: None,
            phase: HolePhase::Location,
            diameter_expression: None,
        });
        self.gizmo_readout = Some((
            "点击孔位置".to_owned(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
        self.changed(window, cx);
    }

    /// Arms neutral-plane picking for selected same-body faces.
    pub fn activate_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = {
            let document = self.document.read(cx);
            let faces: Vec<_> = document
                .selection
                .items
                .iter()
                .filter_map(|item| match *item {
                    SelItem::Face(body, face) => Some((body, face)),
                    _ => None,
                })
                .collect();
            faces.first().and_then(|(body, _)| {
                faces
                    .iter()
                    .all(|(candidate, _)| candidate == body)
                    .then(|| {
                        (
                            *body,
                            faces.iter().map(|(_, face)| *face).collect::<Vec<_>>(),
                        )
                    })
            })
        };
        let Some((body, faces)) = selected else {
            return;
        };
        self.cancel_feature_interaction();
        self.pending_reference = Some(PendingReference::Draft {
            body,
            faces,
            default: PlaneReference {
                origin: DVec3::ZERO,
                normal: DVec3::Z,
                label: "world XY default",
            },
        });
        self.show_reference_hint();
        self.changed(window, cx);
    }

    /// Arms mirror-plane picking for selected bodies.
    pub fn activate_mirror(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sketch_selection = {
            let document = self.document.read(cx);
            document.active_sketch.and_then(|id| {
                let entities: Vec<_> = document
                    .selection
                    .items
                    .iter()
                    .filter_map(|item| match item {
                        SelItem::SketchEntity(selected_id, entity) if *selected_id == id => {
                            Some(*entity)
                        }
                        _ => None,
                    })
                    .collect();
                (!entities.is_empty()).then_some((id, entities))
            })
        };
        if let Some((id, entities)) = sketch_selection {
            self.cancel_feature_interaction();
            self.pending_reference = Some(PendingReference::SketchMirror { id, entities });
            self.gizmo_readout = Some((
                "选择镜像线".into(),
                self.last_pointer + Vec2::new(14.0, -22.0),
            ));
            self.changed(window, cx);
            return;
        }
        let (ids, inferred) = {
            let document = self.document.read(cx);
            let ids: Vec<_> = document
                .selection
                .items
                .iter()
                .filter_map(|item| match item {
                    SelItem::Body(id) => Some(*id),
                    _ => None,
                })
                .collect();
            let inferred = document
                .selection
                .items
                .iter()
                .copied()
                .find_map(|item| self.plane_from_item(item, cx));
            (ids, inferred)
        };
        if ids.is_empty() {
            return;
        }
        self.cancel_feature_interaction();
        self.pending_reference = Some(PendingReference::Mirror {
            ids,
            default: PlaneReference {
                origin: DVec3::ZERO,
                normal: DVec3::Y,
                label: "world ZX default",
            },
        });
        if let Some(plane) = inferred {
            self.resolve_reference(ReferenceGeometry::Plane(plane), cx);
        } else {
            self.show_reference_hint();
        }
        self.changed(window, cx);
    }

    /// Arms the linear/circular pattern chooser for selected bodies.
    pub fn activate_pattern(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.document.read(cx).selection.items.clone();
        let active_sketch = self.document.read(cx).active_sketch;
        if let Some(id) = active_sketch {
            let entities: Vec<_> = selection
                .iter()
                .filter_map(|item| match item {
                    SelItem::SketchEntity(selected_id, entity) if *selected_id == id => {
                        Some(*entity)
                    }
                    _ => None,
                })
                .collect();
            if !entities.is_empty() {
                self.cancel_feature_interaction();
                self.sketch_pattern_interaction = Some(SketchPatternInteraction {
                    id,
                    entities,
                    mode: PatternMode::Linear,
                    count: 3,
                    spacing: 25.0,
                    anchor: None,
                    direction: glam::DVec2::X,
                });
                self.gizmo_readout = Some((
                    "线性阵列：拖动方向和间距".into(),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
                self.changed(window, cx);
                return;
            }
        }
        let ids: Vec<_> = selection
            .iter()
            .filter_map(|item| match item {
                SelItem::Body(id) => Some(*id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        self.cancel_feature_interaction();
        self.pattern_interaction = Some(PatternInteraction {
            ids,
            mode: PatternMode::Linear,
            count: 3,
            spacing: 25.0,
            axis: None,
            start_x: None,
            moved: false,
            expression: None,
        });
        self.pending_reference = Some(PendingReference::Pattern);
        if let Some(axis) = selection
            .into_iter()
            .find_map(|item| self.axis_from_item(item, cx))
        {
            self.resolve_reference(ReferenceGeometry::Axis(axis), cx);
        } else {
            self.show_reference_hint();
        }
        self.changed(window, cx);
    }

    fn cancel_feature_interaction(&mut self) {
        self.revolve_interaction = None;
        self.hole_interaction = None;
        self.draft_interaction = None;
        self.pattern_interaction = None;
        self.sketch_pattern_interaction = None;
        self.helix_interaction = None;
        self.thread_interaction = None;
        self.project_pending = false;
        self.pending_reference = None;
        self.gizmo_readout = None;
    }

    fn begin_feature_drag(&mut self, pointer: Vec2) -> bool {
        if let Some(interaction) = &mut self.revolve_interaction {
            interaction.start_x = Some(pointer.x);
            interaction.moved = false;
            return true;
        }
        if let Some(interaction) = &mut self.draft_interaction {
            interaction.start_x = Some(pointer.x);
            return true;
        }
        if let Some(interaction) = &mut self.pattern_interaction {
            if interaction.axis.is_none() {
                return false;
            }
            interaction.start_x = Some(pointer.x);
            interaction.moved = false;
            return true;
        }
        false
    }

    fn begin_hole_drag(&mut self, pointer: Vec2) -> bool {
        let Some(interaction) = &mut self.hole_interaction else {
            return false;
        };
        if interaction.at.is_none() {
            let (origin, ray) = self.camera.unproject_ray(pointer);
            let normal = interaction.plane.normal().as_vec3();
            let denominator = ray.dot(normal);
            if denominator.abs() < 1.0e-6 {
                return true;
            }
            let distance = (interaction.plane.origin.as_vec3() - origin).dot(normal) / denominator;
            if distance < 0.0 {
                return true;
            }
            interaction.at = Some((origin + ray * distance).as_dvec3());
            interaction.phase = HolePhase::Diameter;
        }
        interaction.start_x = Some(pointer.x);
        self.gizmo_readout = Some((
            format!("⌀ {:.1}", interaction.diameter),
            pointer + Vec2::new(14.0, -22.0),
        ));
        true
    }

    fn update_hole_drag(&mut self, pointer: Vec2) -> bool {
        let Some(interaction) = &mut self.hole_interaction else {
            return false;
        };
        let Some(start_x) = interaction.start_x else {
            return false;
        };
        let delta = f64::from(pointer.x - start_x) * 0.2;
        match interaction.phase {
            HolePhase::Diameter => interaction.diameter = (10.0 + delta).max(0.1),
            HolePhase::Depth => {
                interaction.kind = HoleKind::Blind {
                    depth: (15.0 + delta).max(0.1).into(),
                }
            }
            HolePhase::CounterboreDiameter => {
                let depth = match &interaction.cut {
                    HoleCut::Counterbore { depth, .. } => depth.clone(),
                    _ => 3.0.into(),
                };
                interaction.cut = HoleCut::Counterbore {
                    diameter: (16.0 + delta).max(interaction.diameter + 0.1).into(),
                    depth,
                };
            }
            HolePhase::CounterboreDepth => {
                let diameter = match &interaction.cut {
                    HoleCut::Counterbore { diameter, .. } => diameter.clone(),
                    _ => (interaction.diameter + 2.0).into(),
                };
                interaction.cut = HoleCut::Counterbore {
                    diameter,
                    depth: (3.0 + delta).max(0.1).into(),
                };
            }
            HolePhase::CountersinkDiameter => {
                interaction.cut = HoleCut::Countersink {
                    diameter: (16.0 + delta).max(interaction.diameter + 0.1).into(),
                    angle_degrees: 90.0.into(),
                };
            }
            HolePhase::Location => return false,
        }
        let value = match (interaction.phase, &interaction.kind, &interaction.cut) {
            (HolePhase::Diameter, _, _) => interaction.diameter,
            (HolePhase::Depth, HoleKind::Blind { depth }, _) => depth.value,
            (HolePhase::CounterboreDiameter, _, HoleCut::Counterbore { diameter, .. }) => {
                diameter.value
            }
            (HolePhase::CounterboreDepth, _, HoleCut::Counterbore { depth, .. }) => depth.value,
            (HolePhase::CountersinkDiameter, _, HoleCut::Countersink { diameter, .. }) => {
                diameter.value
            }
            _ => 0.0,
        };
        self.gizmo_readout = Some((
            if matches!(
                interaction.phase,
                HolePhase::Diameter
                    | HolePhase::CounterboreDiameter
                    | HolePhase::CountersinkDiameter
            ) {
                format!("⌀ {value:.1}")
            } else {
                format!("深度 {value:.1}")
            },
            pointer + Vec2::new(14.0, -22.0),
        ));
        self.update_hole_preview();
        true
    }

    fn update_hole_preview(&mut self) {
        let preview = self.hole_interaction.as_ref().and_then(|interaction| {
            let at = interaction.at?;
            let depth = match &interaction.kind {
                HoleKind::Through => 1000.0,
                HoleKind::Blind { depth } => depth.value,
            };
            let shape = occt::Shape::cylinder(
                at + interaction.plane.normal() * 0.01,
                interaction.diameter * 0.5,
                -interaction.plane.normal(),
                depth + 0.01,
            )
            .ok()?;
            Some((tessellate(&shape, 0.5), Mat4::IDENTITY))
        });
        self.renderer.set_preview_mesh(preview);
    }

    fn finish_hole_drag(&mut self, commit: bool, cx: &mut Context<Self>) -> bool {
        let Some(interaction) = &mut self.hole_interaction else {
            return false;
        };
        if !commit {
            self.hole_interaction = None;
            self.gizmo_readout = None;
            self.renderer.set_preview_mesh(None);
            return true;
        }
        interaction.start_x = None;
        interaction.phase = match interaction.phase {
            HolePhase::Diameter => match interaction.kind {
                HoleKind::Blind { .. } => HolePhase::Depth,
                HoleKind::Through => match interaction.cut {
                    HoleCut::Counterbore { .. } => HolePhase::CounterboreDiameter,
                    HoleCut::Countersink { .. } => HolePhase::CountersinkDiameter,
                    HoleCut::None => {
                        let interaction = self.hole_interaction.take().expect("checked");
                        return self.commit_hole(interaction, cx);
                    }
                },
            },
            HolePhase::Depth => match interaction.cut {
                HoleCut::Counterbore { .. } => HolePhase::CounterboreDiameter,
                HoleCut::Countersink { .. } => HolePhase::CountersinkDiameter,
                HoleCut::None => {
                    let interaction = self.hole_interaction.take().expect("checked");
                    return self.commit_hole(interaction, cx);
                }
            },
            HolePhase::CounterboreDiameter => HolePhase::CounterboreDepth,
            HolePhase::CounterboreDepth | HolePhase::CountersinkDiameter => {
                let interaction = self.hole_interaction.take().expect("checked");
                return self.commit_hole(interaction, cx);
            }
            HolePhase::Location => return true,
        };
        self.gizmo_readout = Some((
            match interaction.phase {
                HolePhase::Depth | HolePhase::CounterboreDepth => "拖动或输入深度",
                _ => "拖动或输入直径",
            }
            .to_owned(),
            self.last_pointer + Vec2::new(14.0, -22.0),
        ));
        true
    }

    fn commit_hole(&mut self, interaction: HoleInteraction, cx: &mut Context<Self>) -> bool {
        self.gizmo_readout = None;
        self.renderer.set_preview_mesh(None);
        if let Some(at) = interaction.at {
            self.document.update(cx, |document, cx| {
                if document.apply_hole(
                    interaction.body,
                    interaction.face_index,
                    at,
                    interaction.diameter,
                    interaction.kind,
                    interaction.cut,
                ) {
                    if let Some(expression) = interaction.diameter_expression {
                        document.set_last_history_num_expression(0, expression);
                    }
                    cx.notify();
                }
            });
        }
        true
    }

    fn begin_sketch_pattern(&mut self, pointer: Vec2, cx: &mut Context<Self>) -> bool {
        let Some(local) = self.sketch_local_at(pointer) else {
            return false;
        };
        let Some(interaction) = &mut self.sketch_pattern_interaction else {
            return false;
        };
        if interaction.mode == PatternMode::Circular {
            let (id, entities, count) = (
                interaction.id,
                interaction.entities.clone(),
                interaction.count,
            );
            self.document.update(cx, |document, cx| {
                if document.pattern_sketch_circular(id, &entities, local, count) {
                    cx.notify();
                }
            });
            self.sketch_pattern_interaction = None;
            self.gizmo_readout = None;
            self.sync_sketch_gpu(cx);
        } else {
            interaction.anchor = Some(local);
            interaction.spacing = 0.0;
        }
        true
    }

    fn update_sketch_pattern(&mut self, pointer: Vec2) -> bool {
        let Some(local) = self.sketch_local_at(pointer) else {
            return false;
        };
        let Some(interaction) = &mut self.sketch_pattern_interaction else {
            return false;
        };
        let Some(anchor) = interaction.anchor else {
            return false;
        };
        let delta = local - anchor;
        if delta.length_squared() > 1.0e-12 {
            interaction.direction = delta.normalize();
            interaction.spacing = delta.length();
        }
        self.gizmo_readout = Some((
            format!(
                "线性阵列 {} × {:.2}",
                interaction.count, interaction.spacing
            ),
            pointer + Vec2::new(14.0, -22.0),
        ));
        true
    }

    fn finish_sketch_pattern(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(interaction) = self.sketch_pattern_interaction.take() else {
            return false;
        };
        if interaction.mode == PatternMode::Linear && interaction.spacing > 1.0e-9 {
            self.document.update(cx, |document, cx| {
                if document.pattern_sketch_linear(
                    interaction.id,
                    &interaction.entities,
                    interaction.direction,
                    interaction.count,
                    interaction.spacing,
                ) {
                    cx.notify();
                }
            });
            self.sync_sketch_gpu(cx);
        }
        self.gizmo_readout = None;
        true
    }

    fn update_feature_drag(&mut self, pointer: Vec2) -> bool {
        if let Some(interaction) = &mut self.revolve_interaction {
            let Some(start_x) = interaction.start_x else {
                return false;
            };
            let delta = pointer.x - start_x;
            if delta.abs() > 1.0 {
                interaction.moved = true;
            }
            if interaction.moved {
                interaction.angle_degrees = (f64::from(delta.abs()) * 2.0).min(360.0);
            }
            self.gizmo_readout = Some((
                format!(
                    "axis: {} · {:.0}°",
                    interaction.axis.label, interaction.angle_degrees
                ),
                pointer + Vec2::new(14.0, -22.0),
            ));
            return true;
        }
        if let Some(interaction) = &mut self.draft_interaction {
            let Some(start_x) = interaction.start_x else {
                return false;
            };
            interaction.angle_degrees = (f64::from(pointer.x - start_x) * 0.1).clamp(-45.0, 45.0);
            self.gizmo_readout = Some((
                format!("{:.1}°", interaction.angle_degrees),
                pointer + Vec2::new(14.0, -22.0),
            ));
            return true;
        }
        if let Some(interaction) = &mut self.pattern_interaction {
            let Some(start_x) = interaction.start_x else {
                return false;
            };
            let delta = pointer.x - start_x;
            if delta.abs() > 1.0 {
                interaction.moved = true;
            }
            if interaction.mode == PatternMode::Linear && interaction.moved {
                interaction.spacing = f64::from(delta.abs()) * 0.5;
            }
            self.gizmo_readout = Some((
                match interaction.mode {
                    PatternMode::Linear => format!(
                        "axis: {} · {:.1}",
                        interaction.axis.expect("resolved pattern axis").label,
                        interaction.spacing
                    ),
                    PatternMode::Circular => format!(
                        "axis: {} · 360° / {}",
                        interaction.axis.expect("resolved pattern axis").label,
                        interaction.count
                    ),
                },
                pointer + Vec2::new(14.0, -22.0),
            ));
            return true;
        }
        false
    }

    fn finish_feature_drag(&mut self, commit: bool, cx: &mut Context<Self>) -> bool {
        if let Some(interaction) = self.revolve_interaction.take() {
            self.gizmo_readout = None;
            if commit && interaction.angle_degrees.abs() >= 1.0e-6 {
                self.document.update(cx, |document, cx| {
                    let applied = match &interaction.source {
                        RevolveSource::Profile(source) => document
                            .apply_revolve(
                                *source,
                                interaction.axis.origin,
                                interaction.axis.direction,
                                interaction.angle_degrees,
                                interaction.mode,
                            )
                            .is_some(),
                        RevolveSource::OpenChain {
                            sketch,
                            entity_indices,
                        } => document
                            .apply_open_chain_revolve(
                                *sketch,
                                entity_indices,
                                interaction.axis.origin,
                                interaction.axis.direction,
                                interaction.angle_degrees,
                            )
                            .is_some(),
                    };
                    if applied {
                        if let Some(expression) = interaction.expression {
                            document.set_last_history_num_expression(0, expression);
                        }
                        cx.notify();
                    }
                });
            }
            return true;
        }
        if let Some(interaction) = self.draft_interaction.take() {
            self.gizmo_readout = None;
            if commit && interaction.angle_degrees.abs() >= 1.0e-6 {
                self.document.update(cx, |document, cx| {
                    if document.apply_draft(
                        interaction.body,
                        &interaction.faces,
                        interaction.neutral.normal,
                        interaction.neutral.origin,
                        interaction.neutral.normal,
                        interaction.angle_degrees,
                    ) {
                        if let Some(expression) = interaction.expression {
                            document.set_last_history_num_expression(0, expression);
                        }
                        cx.notify();
                    }
                });
            }
            return true;
        }
        if let Some(interaction) = self.pattern_interaction.take() {
            self.gizmo_readout = None;
            if commit {
                self.document.update(cx, |document, cx| {
                    let axis = interaction
                        .axis
                        .expect("pattern reference resolved before drag");
                    let copies = match interaction.mode {
                        PatternMode::Linear => document.apply_linear_pattern(
                            &interaction.ids,
                            axis.origin,
                            axis.direction,
                            interaction.count,
                            interaction.spacing,
                        ),
                        PatternMode::Circular => document.apply_circular_pattern(
                            &interaction.ids,
                            axis.origin,
                            axis.direction,
                            interaction.count,
                        ),
                    };
                    if !copies.is_empty() {
                        if let Some(expression) = interaction.expression {
                            document.set_last_history_num_expression(0, expression);
                        }
                        cx.notify();
                    }
                });
            }
            return true;
        }
        false
    }

    fn set_pattern_mode(&mut self, mode: PatternMode, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(interaction) = &mut self.sketch_pattern_interaction {
            interaction.mode = mode;
            interaction.count = if mode == PatternMode::Linear { 3 } else { 4 };
            interaction.anchor = None;
            self.gizmo_readout = Some((
                if mode == PatternMode::Linear {
                    "线性阵列：拖动方向和间距"
                } else {
                    "环形阵列：点击中心"
                }
                .into(),
                self.last_pointer + Vec2::new(14.0, -22.0),
            ));
            cx.stop_propagation();
            self.changed(window, cx);
            return;
        }
        if let Some(interaction) = &mut self.pattern_interaction {
            interaction.mode = mode;
            interaction.count = match mode {
                PatternMode::Linear => 3,
                PatternMode::Circular => 6,
            };
            interaction.axis = None;
            self.pending_reference = Some(PendingReference::Pattern);
            self.show_reference_hint();
        }
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn change_pattern_count(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(interaction) = &mut self.sketch_pattern_interaction {
            interaction.count = interaction.count.saturating_add_signed(delta).clamp(2, 12);
            cx.stop_propagation();
            self.changed(window, cx);
            return;
        }
        if let Some(interaction) = &mut self.pattern_interaction {
            let maximum = if interaction.mode == PatternMode::Linear {
                12
            } else {
                24
            };
            interaction.count = interaction
                .count
                .saturating_add_signed(delta)
                .clamp(2, maximum);
            if let Some(axis) = interaction.axis {
                self.gizmo_readout = Some((
                    match interaction.mode {
                        PatternMode::Linear => {
                            format!("axis: {} · {:.1}", axis.label, interaction.spacing)
                        }
                        PatternMode::Circular => {
                            format!("axis: {} · 360° / {}", axis.label, interaction.count)
                        }
                    },
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            }
        }
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn begin_dressup_drag(&mut self, pointer: Vec2, cx: &Context<Self>) -> bool {
        let Some(tool @ (ToolId::Fillet | ToolId::Chamfer)) = self.active_drag_tool else {
            return false;
        };
        let Some((body, edge_indices, origin, direction, bbox_diagonal)) =
            self.selected_dressup(cx)
        else {
            return false;
        };
        let ray = self.camera.unproject_ray(pointer);
        if !hit_test_axis(
            ray,
            origin.as_vec3(),
            direction.as_vec3(),
            self.gizmo_scale(origin.as_vec3()),
        ) {
            return false;
        }
        self.dressup_drag = Some(DressUpInteraction {
            drag: DressUpDrag {
                body,
                edge_indices,
                origin,
                direction,
                radius: 0.01,
                end_radius: None,
                fillet: tool == ToolId::Fillet,
            },
            anchor: cursor_distance(ray.0, ray.1, origin, direction),
            bbox_diagonal,
            last_preview_radius: None,
            expression: None,
            variable_start_entered: false,
        });
        self.hovered_extrude_arrow = true;
        true
    }

    fn update_dressup_drag(&mut self, pointer: Vec2, cx: &Context<Self>) {
        let ray = self.camera.unproject_ray(pointer);
        let Some(interaction) = self.dressup_drag.as_mut() else {
            return;
        };
        let radius = (cursor_distance(
            ray.0,
            ray.1,
            interaction.drag.origin,
            interaction.drag.direction,
        ) - interaction.anchor)
            .max(0.01);
        interaction.drag.radius = radius;
        if self.variable_fillet {
            interaction.drag.end_radius = Some(radius);
        }
        let prefix = if interaction.drag.fillet { 'r' } else { 'c' };
        self.gizmo_readout = Some((
            format!("{prefix} {radius:.1}"),
            self.camera
                .project((interaction.drag.origin + interaction.drag.direction * radius).as_vec3())
                + Vec2::new(14.0, -22.0),
        ));
        let threshold = (interaction.bbox_diagonal * 0.005).max(0.25);
        if interaction
            .last_preview_radius
            .is_some_and(|previous| (radius - previous).abs() < threshold)
        {
            return;
        }
        interaction.last_preview_radius = Some(radius);
        let preview = dressup_preview(self.document.read(cx), &interaction.drag)
            .map(|shape| (tessellate(&shape, 0.5), Mat4::IDENTITY));
        self.renderer
            .set_suppressed_body(preview.as_ref().map(|_| interaction.drag.body));
        self.renderer.set_preview_mesh(preview);
    }

    fn finish_dressup_drag(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(interaction) = self.dressup_drag.take() else {
            return;
        };
        self.renderer.set_preview_mesh(None);
        self.renderer.set_suppressed_body(None);
        self.gizmo_readout = None;
        self.active_drag_tool = None;
        if commit {
            let operation = if interaction.drag.fillet {
                DressUp::Fillet {
                    radius: interaction.drag.radius,
                    end_radius: interaction.drag.end_radius,
                    edge_indices: interaction.drag.edge_indices,
                }
            } else {
                DressUp::Chamfer {
                    radius: interaction.drag.radius,
                    edge_indices: interaction.drag.edge_indices,
                }
            };
            let body = interaction.drag.body;
            self.document.update(cx, |document, cx| {
                if document.apply_dressup(body, operation) {
                    if let Some(expression) = interaction.expression {
                        document.set_last_history_num_expression(0, expression);
                    }
                    cx.notify();
                }
            });
        }
    }

    fn begin_shell_drag(&mut self, pointer: Vec2, cx: &Context<Self>) -> bool {
        if self.active_drag_tool != Some(ToolId::Shell) {
            return false;
        }
        let Some((body, face_indices, origin, direction, bbox_diagonal)) = self.selected_shell(cx)
        else {
            return false;
        };
        let ray = self.camera.unproject_ray(pointer);
        if !hit_test_axis(
            ray,
            origin.as_vec3(),
            direction.as_vec3(),
            self.gizmo_scale(origin.as_vec3()),
        ) {
            return false;
        }
        self.shell_drag = Some(ShellInteraction {
            drag: ShellDrag {
                body,
                face_indices,
                origin,
                direction,
                thickness: 2.0,
            },
            anchor: cursor_distance(ray.0, ray.1, origin, direction),
            bbox_diagonal,
            last_preview_thickness: None,
            expression: None,
        });
        self.hovered_extrude_arrow = true;
        true
    }

    fn update_shell_drag(&mut self, pointer: Vec2, cx: &Context<Self>) {
        let ray = self.camera.unproject_ray(pointer);
        let Some(interaction) = self.shell_drag.as_mut() else {
            return;
        };
        let thickness =
            (2.0 + cursor_distance(
                ray.0,
                ray.1,
                interaction.drag.origin,
                interaction.drag.direction,
            ) - interaction.anchor)
                .max(0.05);
        interaction.drag.thickness = thickness;
        self.gizmo_readout = Some((
            format!("t {thickness:.1}"),
            self.camera.project(
                (interaction.drag.origin + interaction.drag.direction * thickness).as_vec3(),
            ) + Vec2::new(14.0, -22.0),
        ));
        let threshold = (interaction.bbox_diagonal * 0.005).max(0.25);
        if interaction
            .last_preview_thickness
            .is_some_and(|previous| (thickness - previous).abs() < threshold)
        {
            return;
        }
        interaction.last_preview_thickness = Some(thickness);
        let preview = shell_preview(self.document.read(cx), &interaction.drag)
            .map(|shape| (tessellate(&shape, 0.5), Mat4::IDENTITY));
        self.renderer
            .set_suppressed_body(preview.as_ref().map(|_| interaction.drag.body));
        self.renderer.set_preview_mesh(preview);
    }

    fn finish_shell_drag(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(interaction) = self.shell_drag.take() else {
            return;
        };
        self.renderer.set_preview_mesh(None);
        self.renderer.set_suppressed_body(None);
        self.gizmo_readout = None;
        self.active_drag_tool = None;
        if commit {
            let body = interaction.drag.body;
            let faces = interaction.drag.face_indices;
            let thickness = interaction.drag.thickness;
            self.document.update(cx, |document, cx| {
                if document.apply_shell(body, &faces, thickness) {
                    if let Some(expression) = interaction.expression {
                        document.set_last_history_num_expression(0, expression);
                    }
                    cx.notify();
                }
            });
        }
    }

    fn begin_thicken_drag(&mut self, pointer: Vec2, cx: &Context<Self>) -> bool {
        if self.active_drag_tool != Some(ToolId::Thicken) {
            return false;
        }
        let Some((body, origin, direction, bbox_diagonal)) = self.selected_thicken(cx) else {
            return false;
        };
        let ray = self.camera.unproject_ray(pointer);
        if !hit_test_axis(
            ray,
            origin.as_vec3(),
            direction.as_vec3(),
            self.gizmo_scale(origin.as_vec3()),
        ) {
            return false;
        }
        self.thicken_drag = Some(ThickenInteraction {
            body,
            origin,
            direction,
            thickness: 2.0,
            anchor: cursor_distance(ray.0, ray.1, origin, direction),
            bbox_diagonal,
            last_preview_thickness: None,
            expression: None,
        });
        self.hovered_extrude_arrow = true;
        true
    }

    fn update_thicken_drag(&mut self, pointer: Vec2, cx: &Context<Self>) {
        let ray = self.camera.unproject_ray(pointer);
        let Some(interaction) = self.thicken_drag.as_mut() else {
            return;
        };
        let delta = cursor_distance(ray.0, ray.1, interaction.origin, interaction.direction)
            - interaction.anchor;
        let thickness = if delta >= -1.99 {
            2.0 + delta
        } else {
            -(-delta - 2.0)
        };
        interaction.thickness = if thickness.abs() < 0.05 {
            0.05 * thickness.signum().max(1.0)
        } else {
            thickness
        };
        self.gizmo_readout = Some((
            format!("t {:.1}", interaction.thickness),
            self.camera.project(
                (interaction.origin + interaction.direction * interaction.thickness).as_vec3(),
            ) + Vec2::new(14.0, -22.0),
        ));
        let threshold = (interaction.bbox_diagonal * 0.005).max(0.25);
        if interaction
            .last_preview_thickness
            .is_some_and(|previous| (interaction.thickness - previous).abs() < threshold)
        {
            return;
        }
        interaction.last_preview_thickness = Some(interaction.thickness);
        let preview = self
            .document
            .read(cx)
            .bodies
            .iter()
            .find(|body| body.id == interaction.body)
            .and_then(|body| body.shape.thicken(interaction.thickness).ok())
            .map(|shape| (tessellate(&shape, 0.5), Mat4::IDENTITY));
        self.renderer
            .set_suppressed_body(preview.as_ref().map(|_| interaction.body));
        self.renderer.set_preview_mesh(preview);
    }

    fn finish_thicken_drag(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(interaction) = self.thicken_drag.take() else {
            return;
        };
        self.renderer.set_preview_mesh(None);
        self.renderer.set_suppressed_body(None);
        self.gizmo_readout = None;
        self.active_drag_tool = None;
        if commit {
            self.document.update(cx, |document, cx| {
                if document.apply_thicken(interaction.body, interaction.thickness) {
                    if let Some(expression) = interaction.expression {
                        document.set_last_history_num_expression(0, expression);
                    }
                    cx.notify();
                }
            });
        }
    }

    fn begin_extrude_drag(&mut self, pointer: Vec2, cx: &Context<Self>) -> bool {
        if self.active_drag_tool.is_some() && self.active_drag_tool != Some(ToolId::OffsetFace) {
            return false;
        }
        let ray = self.camera.unproject_ray(pointer);
        let face = if self.active_drag_tool == Some(ToolId::OffsetFace) {
            self.selected_offset_face(cx)
        } else {
            self.selected_extrude_face(cx)
        };
        if let Some((body, face_index, origin, normal, bbox_diagonal)) = face {
            if !hit_test_axis(
                ray,
                origin.as_vec3(),
                normal.as_vec3(),
                self.gizmo_scale(origin.as_vec3()),
            ) {
                return false;
            }
            let anchor = cursor_distance(ray.0, ray.1, origin, normal);
            self.extrude_drag = Some(ExtrudeInteraction {
                drag: ExtrudeDrag {
                    body,
                    face_index,
                    origin,
                    normal,
                    distance: 0.0,
                    opposite_distance: 0.0,
                    side_mode: self.extrude_side_mode,
                    mode: if self.active_drag_tool == Some(ToolId::OffsetFace) {
                        ExtrudeMode::Auto
                    } else {
                        self.extrude_mode
                    },
                },
                anchor,
                bbox_diagonal,
                last_preview_distance: None,
                opposite_phase: false,
                expressions: [None, None],
            });
            self.hovered_extrude_arrow = true;
            return true;
        }
        if let Some((sketch, entity_indices, origin, normal, bbox_diagonal)) =
            self.selected_open_chain(cx)
        {
            if !hit_test_axis(
                ray,
                origin.as_vec3(),
                normal.as_vec3(),
                self.gizmo_scale(origin.as_vec3()),
            ) {
                return false;
            }
            let anchor = cursor_distance(ray.0, ray.1, origin, normal);
            self.open_chain_extrude_drag = Some(OpenChainExtrudeInteraction {
                drag: OpenChainExtrudeDrag {
                    sketch,
                    entity_indices,
                    origin,
                    normal,
                    distance: 0.0,
                    opposite_distance: 0.0,
                    side_mode: self.extrude_side_mode,
                },
                anchor,
                bbox_diagonal,
                last_preview_distance: None,
                opposite_phase: false,
                expressions: [None, None],
            });
            self.hovered_extrude_arrow = true;
            return true;
        }
        let Some((sketch, profile_index, origin, normal, bbox_diagonal)) =
            self.selected_extrude_profile(cx)
        else {
            return false;
        };
        if !hit_test_axis(
            ray,
            origin.as_vec3(),
            normal.as_vec3(),
            self.gizmo_scale(origin.as_vec3()),
        ) {
            return false;
        }
        let anchor = cursor_distance(ray.0, ray.1, origin, normal);
        self.profile_extrude_drag = Some(ProfileExtrudeInteraction {
            drag: ProfileExtrudeDrag {
                sketch,
                profile_index,
                origin,
                normal,
                distance: 0.0,
                opposite_distance: 0.0,
                side_mode: self.extrude_side_mode,
                mode: self.extrude_mode,
            },
            anchor,
            bbox_diagonal,
            last_preview_distance: None,
            opposite_phase: false,
            expressions: [None, None],
        });
        self.hovered_extrude_arrow = true;
        true
    }

    fn update_extrude_drag(&mut self, pointer: Vec2, cx: &Context<Self>) {
        let ray = self.camera.unproject_ray(pointer);
        if let Some(interaction) = self.open_chain_extrude_drag.as_mut() {
            let axis = if interaction.opposite_phase {
                -interaction.drag.normal
            } else {
                interaction.drag.normal
            };
            let distance =
                cursor_distance(ray.0, ray.1, interaction.drag.origin, axis) - interaction.anchor;
            if interaction.opposite_phase {
                interaction.drag.opposite_distance = distance.abs();
            } else {
                interaction.drag.distance = distance;
            }
            self.gizmo_readout = Some((
                if interaction.drag.side_mode == ExtrudeSideMode::TwoSided {
                    format!(
                        "A {:.1} / B {:.1}",
                        interaction.drag.distance.abs(),
                        interaction.drag.opposite_distance.abs()
                    )
                } else {
                    format!("{distance:.1}")
                },
                self.camera
                    .project((interaction.drag.origin + axis * distance).as_vec3())
                    + Vec2::new(14.0, -22.0),
            ));
            let threshold = (interaction.bbox_diagonal * 0.005).max(0.25);
            if interaction
                .last_preview_distance
                .is_some_and(|previous| (distance - previous).abs() < threshold)
            {
                return;
            }
            interaction.last_preview_distance = Some(distance);
            let preview = open_chain_prism(self.document.read(cx), &interaction.drag)
                .map(|shape| (tessellate(&shape, 0.5), Mat4::IDENTITY));
            self.renderer.set_preview_mesh(preview);
            return;
        }
        if let Some(interaction) = self.profile_extrude_drag.as_mut() {
            let axis = if interaction.opposite_phase {
                -interaction.drag.normal
            } else {
                interaction.drag.normal
            };
            let distance =
                cursor_distance(ray.0, ray.1, interaction.drag.origin, axis) - interaction.anchor;
            if interaction.opposite_phase {
                interaction.drag.opposite_distance = distance.abs();
            } else {
                interaction.drag.distance = distance;
            }
            self.gizmo_readout = Some((
                if interaction.drag.side_mode == ExtrudeSideMode::TwoSided {
                    format!(
                        "A {:.1} / B {:.1}",
                        interaction.drag.distance.abs(),
                        interaction.drag.opposite_distance.abs()
                    )
                } else {
                    format!("{distance:.1}")
                },
                self.camera
                    .project((interaction.drag.origin + axis * distance).as_vec3())
                    + Vec2::new(14.0, -22.0),
            ));
            let threshold = (interaction.bbox_diagonal * 0.005).max(0.25);
            if interaction
                .last_preview_distance
                .is_some_and(|previous| (distance - previous).abs() < threshold)
            {
                return;
            }
            interaction.last_preview_distance = Some(distance);
            let preview = profile_prism(self.document.read(cx), &interaction.drag)
                .map(|shape| (tessellate(&shape, 0.5), Mat4::IDENTITY));
            self.renderer.set_preview_mesh(preview);
            return;
        }
        let Some(interaction) = self.extrude_drag.as_mut() else {
            return;
        };
        let axis = if interaction.opposite_phase {
            -interaction.drag.normal
        } else {
            interaction.drag.normal
        };
        let distance =
            cursor_distance(ray.0, ray.1, interaction.drag.origin, axis) - interaction.anchor;
        if interaction.opposite_phase {
            interaction.drag.opposite_distance = distance.abs();
        } else {
            interaction.drag.distance = distance;
        }
        self.gizmo_readout = Some((
            if interaction.drag.side_mode == ExtrudeSideMode::TwoSided {
                format!(
                    "A {:.1} / B {:.1}",
                    interaction.drag.distance.abs(),
                    interaction.drag.opposite_distance.abs()
                )
            } else {
                format!("{distance:.1}")
            },
            self.camera
                .project((interaction.drag.origin + axis * distance).as_vec3())
                + Vec2::new(14.0, -22.0),
        ));

        let threshold = (interaction.bbox_diagonal * 0.005).max(0.25);
        let should_update = interaction
            .last_preview_distance
            .is_none_or(|previous| (distance - previous).abs() >= threshold)
            || distance.abs() < 1.0e-6;
        if !should_update {
            return;
        }
        interaction.last_preview_distance = Some(distance);
        let drag = interaction.drag;
        let preview = {
            let document = self.document.read(cx);
            extrude_prism(document, &drag).map(|shape| (tessellate(&shape, 0.5), Mat4::IDENTITY))
        };
        self.renderer.set_preview_mesh(preview);
    }

    fn finish_extrude_drag(&mut self, commit: bool, cx: &mut Context<Self>) {
        if commit
            && let Some(interaction) = self.open_chain_extrude_drag.as_mut()
            && interaction.drag.side_mode == ExtrudeSideMode::TwoSided
            && !interaction.opposite_phase
            && interaction.drag.distance.abs() >= 1.0e-6
        {
            let ray = self.camera.unproject_ray(self.last_pointer);
            interaction.anchor = cursor_distance(
                ray.0,
                ray.1,
                interaction.drag.origin,
                -interaction.drag.normal,
            );
            interaction.opposite_phase = true;
            interaction.last_preview_distance = None;
            return;
        }
        if commit
            && let Some(interaction) = self.profile_extrude_drag.as_mut()
            && interaction.drag.side_mode == ExtrudeSideMode::TwoSided
            && !interaction.opposite_phase
            && interaction.drag.distance.abs() >= 1.0e-6
        {
            let ray = self.camera.unproject_ray(self.last_pointer);
            interaction.anchor = cursor_distance(
                ray.0,
                ray.1,
                interaction.drag.origin,
                -interaction.drag.normal,
            );
            interaction.opposite_phase = true;
            interaction.last_preview_distance = None;
            return;
        }
        if commit
            && let Some(interaction) = self.extrude_drag.as_mut()
            && self.active_drag_tool != Some(ToolId::OffsetFace)
            && interaction.drag.side_mode == ExtrudeSideMode::TwoSided
            && !interaction.opposite_phase
            && interaction.drag.distance.abs() >= 1.0e-6
        {
            let ray = self.camera.unproject_ray(self.last_pointer);
            interaction.anchor = cursor_distance(
                ray.0,
                ray.1,
                interaction.drag.origin,
                -interaction.drag.normal,
            );
            interaction.opposite_phase = true;
            interaction.last_preview_distance = None;
            return;
        }
        if let Some(interaction) = self.open_chain_extrude_drag.take() {
            self.renderer.set_preview_mesh(None);
            self.gizmo_readout = None;
            if commit && interaction.drag.distance.abs() >= 1.0e-6 {
                self.retessellate_only = None;
                self.document.update(cx, |document, cx| {
                    if document
                        .apply_open_chain_extrude(
                            interaction.drag.sketch,
                            &interaction.drag.entity_indices,
                            interaction.drag.distance,
                            interaction.drag.opposite_distance,
                            interaction.drag.side_mode,
                        )
                        .is_some()
                    {
                        for (slot, expression) in interaction.expressions.iter().enumerate() {
                            if let Some(expression) = expression {
                                document.set_last_history_num_expression(slot, expression.clone());
                            }
                        }
                        cx.notify();
                    }
                });
            }
            return;
        }
        if let Some(interaction) = self.profile_extrude_drag.take() {
            self.renderer.set_preview_mesh(None);
            self.gizmo_readout = None;
            if commit && interaction.drag.distance.abs() >= 1.0e-6 {
                self.retessellate_only = None;
                self.document.update(cx, |document, cx| {
                    if document.apply_profile_extrude(&interaction.drag) {
                        for (slot, expression) in interaction.expressions.iter().enumerate() {
                            if let Some(expression) = expression {
                                document.set_last_history_num_expression(slot, expression.clone());
                            }
                        }
                        cx.notify();
                    }
                });
            }
            return;
        }
        let Some(interaction) = self.extrude_drag.take() else {
            return;
        };
        let offset_face = self.active_drag_tool == Some(ToolId::OffsetFace);
        if offset_face {
            self.active_drag_tool = None;
        }
        self.renderer.set_preview_mesh(None);
        self.gizmo_readout = None;
        if commit && interaction.drag.distance.abs() >= 1.0e-6 {
            self.retessellate_only = Some(vec![interaction.drag.body]);
            self.document.update(cx, |document, cx| {
                let applied = if offset_face {
                    document.apply_offset_face(&interaction.drag)
                } else {
                    document.apply_extrude(&interaction.drag)
                };
                if applied {
                    for (slot, expression) in interaction.expressions.iter().enumerate() {
                        if let Some(expression) = expression {
                            document.set_last_history_num_expression(slot, expression.clone());
                        }
                    }
                    cx.notify();
                }
            });
        }
    }

    fn set_extrude_mode(&mut self, mode: ExtrudeMode, window: &mut Window, cx: &mut Context<Self>) {
        self.extrude_mode = mode;
        if let Some(interaction) = &mut self.extrude_drag {
            interaction.drag.mode = mode;
        }
        if let Some(interaction) = &mut self.profile_extrude_drag {
            interaction.drag.mode = mode;
        }
        if let Some(interaction) = &mut self.revolve_interaction {
            interaction.mode = mode;
        }
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn set_variable_fillet(&mut self, variable: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.variable_fillet = variable;
        if let Some(interaction) = &mut self.dressup_drag {
            interaction.drag.end_radius = variable.then_some(interaction.drag.radius);
            interaction.variable_start_entered = false;
        }
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn set_extrude_side_mode(
        &mut self,
        mode: ExtrudeSideMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extrude_side_mode = mode;
        if let Some(interaction) = &mut self.extrude_drag {
            interaction.drag.side_mode = mode;
            interaction.drag.opposite_distance = 0.0;
            interaction.opposite_phase = false;
        }
        if let Some(interaction) = &mut self.profile_extrude_drag {
            interaction.drag.side_mode = mode;
            interaction.drag.opposite_distance = 0.0;
            interaction.opposite_phase = false;
        }
        if let Some(interaction) = &mut self.open_chain_extrude_drag {
            interaction.drag.side_mode = mode;
            interaction.drag.opposite_distance = 0.0;
            interaction.opposite_phase = false;
        }
        cx.stop_propagation();
        self.changed(window, cx);
    }

    fn begin_gizmo_drag(&mut self, pointer: Vec2, cx: &Context<Self>) -> bool {
        let Some(pivot) = self.selection_pivot(cx) else {
            return false;
        };
        let ray = self.camera.unproject_ray(pointer);
        let Some(handle) = hit_test(ray, pivot, self.gizmo_scale(pivot)) else {
            return false;
        };
        let anchor = if let Some(axis) = handle.axis() {
            if handle.is_ring() {
                let Some(point) = ray_plane(ray.0, ray.1, pivot, axis) else {
                    return false;
                };
                DragAnchor::Ring((point - pivot).normalize_or_zero())
            } else {
                DragAnchor::Axis(axis_drag_parameter(ray.0, ray.1, pivot, axis))
            }
        } else {
            let normal = (pivot - self.camera.eye()).normalize_or_zero();
            let Some(point) = ray_plane(ray.0, ray.1, pivot, normal) else {
                return false;
            };
            DragAnchor::Center(point)
        };
        let ids = Self::selected_body_ids(self.document.read(cx));
        self.gizmo_drag = Some(GizmoDrag {
            handle,
            pivot,
            ids,
            anchor,
            current: None,
        });
        self.gizmo_repeat = 1;
        self.hovered_gizmo = Some(handle);
        true
    }

    fn update_gizmo_drag(&mut self, pointer: Vec2, shift: bool) {
        let Some(drag) = self.gizmo_drag.as_mut() else {
            return;
        };
        let ray = self.camera.unproject_ray(pointer);
        let (operation, matrix, label, display_pivot) = match drag.anchor {
            DragAnchor::Axis(start) => {
                let axis = drag.handle.axis().expect("axis handle");
                let delta = axis_drag_parameter(ray.0, ray.1, drag.pivot, axis) - start;
                let translation = axis * delta;
                let name = match drag.handle {
                    Handle::AxisX => 'X',
                    Handle::AxisY => 'Y',
                    Handle::AxisZ => 'Z',
                    _ => unreachable!(),
                };
                (
                    TransformOp::Translate(translation.as_dvec3()),
                    Mat4::from_translation(translation),
                    format!("Δ{name} {delta:.1}"),
                    drag.pivot + translation,
                )
            }
            DragAnchor::Center(start) => {
                let normal = (drag.pivot - self.camera.eye()).normalize_or_zero();
                let Some(point) = ray_plane(ray.0, ray.1, drag.pivot, normal) else {
                    return;
                };
                let translation = point - start;
                (
                    TransformOp::Translate(translation.as_dvec3()),
                    Mat4::from_translation(translation),
                    format!("Δ {:.1}", translation.length()),
                    drag.pivot + translation,
                )
            }
            DragAnchor::Ring(start) => {
                let axis = drag.handle.axis().expect("ring axis");
                let Some(point) = ray_plane(ray.0, ray.1, drag.pivot, axis) else {
                    return;
                };
                let current = (point - drag.pivot).normalize_or_zero();
                let angle = snap_angle(
                    axis.dot(start.cross(current)).atan2(start.dot(current)),
                    shift,
                );
                let matrix = Mat4::from_translation(drag.pivot)
                    * Mat4::from_axis_angle(axis, angle)
                    * Mat4::from_translation(-drag.pivot);
                (
                    TransformOp::Rotate {
                        origin: drag.pivot.as_dvec3(),
                        axis: axis.as_dvec3(),
                        angle_rad: f64::from(angle),
                    },
                    matrix,
                    format!("{:.1}°", angle.to_degrees()),
                    drag.pivot,
                )
            }
        };
        drag.current = Some(operation);
        for &id in &drag.ids {
            self.renderer.set_preview_transform(id, matrix);
        }
        self.gizmo_readout = Some((
            label,
            self.camera.project(display_pivot) + Vec2::new(14.0, -22.0),
        ));
    }

    fn finish_gizmo_drag(&mut self, commit: bool, cx: &mut Context<Self>) {
        let Some(drag) = self.gizmo_drag.take() else {
            return;
        };
        self.renderer.clear_preview_transforms();
        self.gizmo_readout = None;
        if commit && let Some(operation) = drag.current {
            let repeat = self.gizmo_repeat;
            self.retessellate_only = (repeat == 1).then(|| drag.ids.clone());
            self.document.update(cx, |document, cx| {
                if repeat == 1 {
                    document.apply_transform(&drag.ids, operation);
                } else {
                    document.apply_multi_transform(&drag.ids, operation, repeat);
                }
                cx.notify();
            });
        }
    }

    fn sync_scene(&mut self, cx: &mut Context<Self>) {
        let pending = self.pending_sketch_lines();
        let document = self.document.read(cx);
        if self.uploaded_epoch == document.scene_epoch {
            return;
        }
        let posed_centers: Vec<_> = document
            .bodies
            .iter()
            .map(|body| {
                let local = body
                    .shape
                    .aabb()
                    .map_or(DVec3::ZERO, |(min, max)| (min + max) * 0.5);
                body.pose.transform_point3(local.as_vec3())
            })
            .collect();
        let assembly_center =
            posed_centers.iter().copied().sum::<Vec3>() / posed_centers.len().max(1) as f32;
        let exploded_factor = self.exploded_factor;
        let render_pose = |index: usize, pose: Mat4| {
            Mat4::from_translation(exploded_offset(
                posed_centers[index],
                assembly_center,
                exploded_factor,
            )) * pose
        };
        let can_update_subset = self.retessellate_only.is_some()
            && self.scene_meshes.len() == document.bodies.len()
            && self
                .scene_meshes
                .iter()
                .zip(&document.bodies)
                .all(|(mesh, body)| mesh.id == body.id);
        if can_update_subset {
            let ids = self.retessellate_only.take().unwrap_or_default();
            for (index, (scene, body)) in self
                .scene_meshes
                .iter_mut()
                .zip(&document.bodies)
                .enumerate()
            {
                scene.visible = body.visible;
                scene.shape = Arc::clone(&body.shape);
                scene.material = body.material;
                scene.kind = body.kind;
                scene.pose = render_pose(index, body.pose);
                if ids.contains(&body.id) {
                    scene.mesh = body_render_mesh(body);
                }
            }
        } else {
            self.retessellate_only = None;
            self.scene_meshes = document
                .bodies
                .iter()
                .enumerate()
                .map(|(index, body)| SceneMesh {
                    id: body.id,
                    visible: body.visible,
                    shape: Arc::clone(&body.shape),
                    mesh: body_render_mesh(body),
                    material: body.material,
                    kind: body.kind,
                    pose: render_pose(index, body.pose),
                })
                .collect();
        }
        let upload = scene_upload_list(&self.scene_meshes, self.isolated.as_ref());
        self.renderer.upload_scene(&upload);
        self.renderer.clear_preview_transforms();
        for body in &self.scene_meshes {
            self.renderer.set_preview_transform(body.id, body.pose);
        }
        self.renderer.upload_sketches(
            &document.sketches,
            &document.construction_planes,
            &document.construction_axes,
            &document.construction_points,
            &pending,
            &document.selection.items,
        );
        self.renderer
            .upload_reference_images(&document.reference_images);
        self.uploaded_epoch = document.scene_epoch;
        let scene_epoch = document.scene_epoch;
        self.hovered = self.hovered.filter(|item| match *item {
            SelItem::Plane(id) => document
                .construction_planes
                .iter()
                .any(|plane| plane.id == id),
            SelItem::Profile(id, index) => document
                .sketches
                .iter()
                .find(|sketch| sketch.id == id)
                .is_some_and(|sketch| index < sketch.profiles().len()),
            SelItem::SketchEntity(id, index) => document
                .sketches
                .iter()
                .find(|sketch| sketch.id == id)
                .is_some_and(|sketch| index < sketch.entities.len()),
            _ => item
                .body_id()
                .is_some_and(|id| document.bodies.iter().any(|body| body.id == id)),
        });
        if self.section_enabled && self.section_interference_epoch != Some(scene_epoch) {
            self.refresh_section_interference(cx);
        }
    }

    fn refresh_section_interference(&mut self, cx: &Context<Self>) {
        if !self.section_enabled {
            return;
        }
        let document = self.document.read(cx);
        let visible: Vec<_> = document.bodies.iter().filter(|body| body.visible).collect();
        let total_pairs = visible
            .len()
            .saturating_mul(visible.len().saturating_sub(1))
            / 2;
        if total_pairs > 6 {
            eprintln!("section interference: capped {total_pairs} visible-body pairs at 6");
        }
        let mut common_shapes = Vec::new();
        let mut tested = 0;
        'pairs: for first in 0..visible.len() {
            for second in first + 1..visible.len() {
                if tested == 6 {
                    break 'pairs;
                }
                tested += 1;
                match visible[first].shape.common(&visible[second].shape) {
                    Ok(shape)
                        if shape
                            .volume_properties()
                            .is_ok_and(|properties| properties.volume.abs() > 1.0e-9) =>
                    {
                        common_shapes.push(shape)
                    }
                    Ok(_) => {}
                    Err(error) => eprintln!("section interference common failed: {error}"),
                }
            }
        }
        let epoch = document.scene_epoch;
        let preview = if common_shapes.is_empty() {
            None
        } else {
            occt::Shape::compound(common_shapes)
                .ok()
                .map(|shape| (tessellate(&shape, 0.2), Mat4::IDENTITY))
        };
        self.renderer.set_interference_mesh(preview);
        self.section_interference_epoch = Some(epoch);
        self.dirty = true;
    }

    fn pick_bodies(&self) -> Vec<PickBody<'_>> {
        self.scene_meshes
            .iter()
            .filter(|body| {
                body.visible
                    && self
                        .isolated
                        .as_ref()
                        .is_none_or(|isolated| isolated.contains(&body.id))
            })
            .map(|body| PickBody {
                id: body.id,
                mesh: &body.mesh,
                shape: &body.shape,
                pose: body.pose,
            })
            .collect()
    }

    fn item_at(
        &self,
        pointer: Vec2,
        double_click: bool,
        filter: SelectionFilter,
    ) -> Option<SelItem> {
        self.pick_candidates(pointer, double_click, filter)
            .first()
            .map(|candidate| candidate.item)
    }

    fn pick_candidates(
        &self,
        pointer: Vec2,
        double_click: bool,
        filter: SelectionFilter,
    ) -> Vec<PickCandidate> {
        let bodies = self.pick_bodies();
        let (origin, ray) = self.camera.unproject_ray(pointer);
        let faces = nearest_face_per_body(&pick_all(&bodies, origin, ray));
        let mut candidates: Vec<_> = match (filter, double_click) {
            (SelectionFilter::Auto, true) | (SelectionFilter::Body, _) => faces
                .iter()
                .map(|hit| PickCandidate {
                    item: SelItem::Body(hit.body),
                    t: hit.t,
                })
                .collect(),
            (SelectionFilter::Auto, false) | (SelectionFilter::Face, _) => faces
                .iter()
                .map(|hit| PickCandidate {
                    item: SelItem::Face(hit.body, hit.face),
                    t: hit.t,
                })
                .collect(),
            (SelectionFilter::Edge, _) => Vec::new(),
        };
        match filter {
            SelectionFilter::Auto if !double_click => {
                if let Some(hit) = pick_edge(&bodies, &self.camera, pointer, 6.0) {
                    let t = faces
                        .iter()
                        .find(|face| face.body == hit.body)
                        .or_else(|| faces.first())
                        .map_or(0.0, |face| face.t);
                    candidates.insert(
                        0,
                        PickCandidate {
                            item: SelItem::Edge(hit.body, hit.edge),
                            t,
                        },
                    );
                }
            }
            SelectionFilter::Edge => {
                if let Some(hit) = pick_edge(&bodies, &self.camera, pointer, 6.0) {
                    candidates.push(PickCandidate {
                        item: SelItem::Edge(hit.body, hit.edge),
                        t: 0.0,
                    });
                }
            }
            SelectionFilter::Auto | SelectionFilter::Body | SelectionFilter::Face => {}
        }
        candidates
    }

    fn click_candidates(
        &self,
        pointer: Vec2,
        double_click: bool,
        filter: SelectionFilter,
    ) -> (Vec<PickCandidate>, bool) {
        let candidates = self.pick_candidates(pointer, double_click, filter);
        let edge_and_face = candidates
            .iter()
            .any(|candidate| matches!(candidate.item, SelItem::Edge(_, _)))
            && candidates
                .iter()
                .any(|candidate| matches!(candidate.item, SelItem::Face(_, _)));
        let diagonal = self
            .scene_bounds()
            .map_or(0.0, |(minimum, maximum)| minimum.distance(maximum));
        let ambiguous = ambiguous_candidates(&candidates, diagonal, edge_and_face);
        let has_ambiguity = ambiguous.len() >= 2;
        (ambiguous, has_ambiguity)
    }

    fn zoom_to_face_at(&mut self, pointer: Vec2, window: &mut Window, cx: &mut Context<Self>) {
        let bodies = self.pick_bodies();
        let (origin, ray) = self.camera.unproject_ray(pointer);
        let Some(hit) = pick_face(&bodies, origin, ray) else {
            return;
        };
        let Some(body) = self.scene_meshes.iter().find(|body| body.id == hit.body) else {
            return;
        };
        let Some(range) = body.mesh.face_ranges.get(hit.face as usize) else {
            return;
        };
        let Some(indices) = body
            .mesh
            .indices
            .get(range.start as usize..range.end as usize)
        else {
            return;
        };
        let mut minimum = Vec3::splat(f32::INFINITY);
        let mut maximum = Vec3::splat(f32::NEG_INFINITY);
        let mut normal = Vec3::ZERO;
        for &index in indices {
            let Some((&position, &vertex_normal)) = body
                .mesh
                .positions
                .get(index as usize)
                .zip(body.mesh.normals.get(index as usize))
            else {
                return;
            };
            let position = Vec3::from(position);
            minimum = minimum.min(position);
            maximum = maximum.max(position);
            normal += Vec3::from(vertex_normal);
        }
        let Some(target) = frame_face_target(
            minimum,
            maximum,
            normal,
            self.camera.viewport_size,
            self.camera.fov,
        ) else {
            return;
        };
        self.camera
            .animate_to_pivot(target.pivot, target.yaw, target.pitch, target.distance);
        self.changed(window, cx);
    }

    fn construction_plane_at(&self, pointer: Vec2, cx: &Context<Self>) -> Option<SelItem> {
        let threshold = 6.0 * self.device_scale.max(1.0);
        let document = self.document.read(cx);
        let mut hits: Vec<(f32, SelItem)> = document
            .construction_planes
            .iter()
            .rev()
            .filter(|plane| plane.visible)
            .filter_map(|plane| {
                let corners = [
                    glam::DVec2::new(-60.0, -60.0),
                    glam::DVec2::new(60.0, -60.0),
                    glam::DVec2::new(60.0, 60.0),
                    glam::DVec2::new(-60.0, 60.0),
                ];
                let distance = (0..4)
                    .map(|index| {
                        point_segment_distance(
                            pointer,
                            self.camera
                                .project(plane.plane.to_world(corners[index]).as_vec3()),
                            self.camera
                                .project(plane.plane.to_world(corners[(index + 1) % 4]).as_vec3()),
                        )
                    })
                    .fold(f32::INFINITY, f32::min);
                (distance <= threshold).then_some((distance, SelItem::Plane(plane.id)))
            })
            .collect();
        hits.extend(
            document
                .construction_axes
                .iter()
                .filter(|axis| axis.visible)
                .filter_map(|axis| {
                    let distance = point_segment_distance(
                        pointer,
                        self.camera
                            .project((axis.origin - axis.direction * 60.0).as_vec3()),
                        self.camera
                            .project((axis.origin + axis.direction * 60.0).as_vec3()),
                    );
                    (distance <= threshold).then_some((distance, SelItem::Axis(axis.id)))
                }),
        );
        hits.extend(
            document
                .construction_points
                .iter()
                .filter(|point| point.visible)
                .filter_map(|point| {
                    let distance = pointer.distance(self.camera.project(point.position.as_vec3()));
                    (distance <= threshold).then_some((distance, SelItem::Point(point.id)))
                }),
        );
        hits.into_iter()
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, item)| item)
    }

    fn projected_body_bounds(&self, body: BodyId, cx: &Context<Self>) -> Option<ScreenRect> {
        let document = self.document.read(cx);
        let body = document
            .bodies
            .iter()
            .find(|candidate| candidate.id == body && candidate.visible)?;
        let (minimum, maximum) = body.shape.aabb().ok()?;
        ScreenRect::from_projected_points(
            [
                DVec3::new(minimum.x, minimum.y, minimum.z),
                DVec3::new(maximum.x, minimum.y, minimum.z),
                DVec3::new(minimum.x, maximum.y, minimum.z),
                DVec3::new(maximum.x, maximum.y, minimum.z),
                DVec3::new(minimum.x, minimum.y, maximum.z),
                DVec3::new(maximum.x, minimum.y, maximum.z),
                DVec3::new(minimum.x, maximum.y, maximum.z),
                DVec3::new(maximum.x, maximum.y, maximum.z),
            ]
            .into_iter()
            .map(|corner| self.camera.project(corner.as_vec3())),
        )
    }

    fn marquee_items(
        &self,
        start: Vec2,
        current: Vec2,
        filter: SelectionFilter,
        cx: &Context<Self>,
    ) -> Vec<SelItem> {
        let selection_rect = ScreenRect::from_points(start, current);
        let mode = marquee_mode(start, current);
        let points_match = |points: &[Vec2]| {
            !points.is_empty()
                && match mode {
                    MarqueeMode::Window => points
                        .iter()
                        .all(|point| selection_rect.contains_point(*point)),
                    MarqueeMode::Crossing => points
                        .iter()
                        .any(|point| selection_rect.contains_point(*point)),
                }
        };
        let mut selected = Vec::new();
        for body in self.scene_meshes.iter().filter(|body| body.visible) {
            match filter {
                SelectionFilter::Auto | SelectionFilter::Body => {
                    let Some(bounds) = self.projected_body_bounds(body.id, cx) else {
                        continue;
                    };
                    let matches = screen_bounds_match(selection_rect, bounds, mode);
                    if matches {
                        selected.push(SelItem::Body(body.id));
                    }
                }
                SelectionFilter::Face => {
                    for (face_index, range) in body.mesh.face_ranges.iter().enumerate() {
                        let points: Vec<_> = body.mesh.indices
                            [range.start as usize..range.end as usize]
                            .iter()
                            .map(|index| {
                                self.camera
                                    .project(Vec3::from(body.mesh.positions[*index as usize]))
                            })
                            .collect();
                        if points_match(&points) {
                            selected.push(SelItem::Face(body.id, face_index as u32));
                        }
                    }
                }
                SelectionFilter::Edge => {
                    for (edge_index, edge) in body.mesh.edges.iter().enumerate() {
                        let points: Vec<_> = edge
                            .points
                            .iter()
                            .map(|point| self.camera.project(Vec3::from(*point)))
                            .collect();
                        if points_match(&points) {
                            selected.push(SelItem::Edge(body.id, edge_index as u32));
                        }
                    }
                }
            }
        }
        selected
    }

    fn finish_marquee(&mut self, commit: bool, cx: &mut Context<Self>) -> bool {
        let Some(marquee) = self.marquee.take() else {
            return false;
        };
        if !commit {
            return true;
        }
        let filter = self.document.read(cx).selection.filter;
        let items = marquee
            .active
            .then(|| self.marquee_items(marquee.start, marquee.current, filter, cx));
        self.document.update(cx, |document, cx| {
            if let Some(items) = items {
                if marquee.shift {
                    for item in items {
                        if !document.selection.items.contains(&item) {
                            document.selection.items.push(item);
                        }
                    }
                } else {
                    document.selection.items = items;
                }
            } else if !marquee.shift {
                document.selection.clear();
            }
            cx.notify();
        });
        true
    }

    fn zoom_anchor(&self, cursor: Vec2, cx: &Context<Self>) -> Vec3 {
        let (origin, ray) = self.camera.unproject_ray(cursor);
        let document = self.document.read(cx);
        let hit = document
            .bodies
            .iter()
            .filter(|body| body.visible)
            .filter_map(|body| {
                body.shape
                    .ray_hits(origin.as_dvec3(), ray.as_dvec3())
                    .ok()?
                    .into_iter()
                    .filter(|hit| hit.t >= 0.0)
                    .min_by(|left, right| left.t.total_cmp(&right.t))
            })
            .min_by(|left, right| left.t.total_cmp(&right.t));
        if let Some(hit) = hit {
            return hit.point.as_vec3();
        }
        if ray.z.abs() > 1.0e-5 {
            let distance = -origin.z / ray.z;
            if distance >= 0.0 {
                return origin + ray * distance;
            }
        }
        self.camera.cursor_plane_point(cursor)
    }

    fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pointer = Self::pointer(event.position, window.scale_factor());
        self.last_pointer = pointer;
        window.focus(&self.focus_handle, cx);
        if event.button == MouseButton::Left && self.pick_popup.take().is_some() {
            self.hovered = None;
            self.changed(window, cx);
            return;
        }
        if self.cube_rect().contains(pointer) {
            cx.stop_propagation();
            self.dragging = None;
            if event.button == MouseButton::Left {
                if event.click_count >= 2 {
                    self.cube_interaction = None;
                    self.go_to_standard_view(StandardView::Iso, window, cx);
                } else {
                    let pressed_region = self.cube_region_at(pointer);
                    self.hovered_cube = pressed_region;
                    self.cube_interaction = Some(CubeInteraction {
                        press_pointer: pointer,
                        pressed_region,
                        dragged: false,
                    });
                    self.changed(window, cx);
                }
            }
            return;
        }
        if event.button == MouseButton::Left {
            if self.begin_joint_drive(pointer, cx) {
                self.changed(window, cx);
                return;
            }
            if let Some(interaction) = &mut self.m6_interaction {
                match interaction {
                    M6Interaction::Scale { factor, drag, .. } => *drag = Some((pointer.x, *factor)),
                    M6Interaction::Split { y, drag, .. } => *drag = Some((pointer.x, *y)),
                    M6Interaction::ConstructionPlane { distance, drag, .. } => {
                        *drag = Some((pointer.x, *distance));
                    }
                    M6Interaction::Align { .. } => {}
                }
                if !matches!(interaction, M6Interaction::Align { .. }) {
                    self.changed(window, cx);
                    return;
                }
            }
            if self.begin_section_drag(pointer) {
                self.changed(window, cx);
                return;
            }
            if self.measure_enabled {
                self.add_measure_anchor(pointer, cx);
                self.changed(window, cx);
                return;
            }
            if self.sketch_pattern_interaction.is_some() {
                self.begin_sketch_pattern(pointer, cx);
                self.changed(window, cx);
                return;
            }
            if self.project_pending {
                let source = self.item_at(pointer, false, SelectionFilter::Auto);
                let sketch = self.document.read(cx).active_sketch;
                if let (Some(sketch), Some(source @ (SelItem::Edge(_, _) | SelItem::Face(_, _)))) =
                    (sketch, source)
                {
                    self.document.update(cx, |document, cx| {
                        if document.project_to_sketch(sketch, source) {
                            cx.notify();
                        }
                    });
                    self.project_pending = false;
                    self.gizmo_readout = Some((
                        "投影已创建为构造几何".to_owned(),
                        self.last_pointer + Vec2::new(14.0, -22.0),
                    ));
                    self.sync_sketch_gpu(cx);
                }
                self.changed(window, cx);
                return;
            }
            if self.joint_tool_active {
                self.pick_joint_connector(pointer, cx);
                self.changed(window, cx);
                return;
            }
            if self.pending_reference.is_some() {
                self.pick_pending_reference(pointer, cx);
                self.changed(window, cx);
                return;
            }
            if self.begin_hole_drag(pointer) {
                self.changed(window, cx);
                return;
            }
            if self.begin_feature_drag(pointer) {
                self.changed(window, cx);
                return;
            }
            if self.begin_dressup_drag(pointer, cx)
                || self.begin_shell_drag(pointer, cx)
                || self.begin_thicken_drag(pointer, cx)
            {
                self.changed(window, cx);
                return;
            }
            if self.begin_extrude_drag(pointer, cx) {
                self.changed(window, cx);
                return;
            }
            if self.begin_gizmo_drag(pointer, cx) {
                self.changed(window, cx);
                return;
            }
            let sketch_tool = self
                .sketch_interaction
                .as_ref()
                .map(|interaction| interaction.tool);
            if matches!(
                sketch_tool,
                Some(ToolId::TwoTangentCircle | ToolId::ThreeTangentCircle)
            ) && let Some(local) = self.sketch_local_at(pointer)
            {
                let id = self.sketch_interaction.as_ref().expect("tool active").id;
                let created = self.document.update(cx, |document, cx| {
                    let Some(sketch) = document.sketches.iter().find(|sketch| sketch.id == id)
                    else {
                        return false;
                    };
                    let selected: Vec<usize> = document
                        .selection
                        .items
                        .iter()
                        .filter_map(|item| match item {
                            SelItem::SketchEntity(selected_id, index) if *selected_id == id => {
                                Some(*index)
                            }
                            _ => None,
                        })
                        .collect();
                    let needed = if sketch_tool == Some(ToolId::TwoTangentCircle) {
                        2
                    } else {
                        3
                    };
                    if selected.len() != needed {
                        return false;
                    }
                    let lines: Option<Vec<_>> = selected
                        .iter()
                        .map(|&index| match sketch.entities.get(index)?.geo {
                            SketchEntity::Line { a, b } => Some((a, b)),
                            _ => None,
                        })
                        .collect();
                    let Some(lines) = lines else { return false };
                    let circle = if needed == 2 {
                        let (a, b) = lines[0];
                        let radius = ((b - a).perp_dot(local - a).abs()
                            / b.distance(a).max(1.0e-12))
                        .max(0.1);
                        two_tangent_circle(a, b, lines[1].0, lines[1].1, radius, local)
                    } else {
                        three_tangent_circle([lines[0], lines[1], lines[2]])
                    };
                    let Some((center, radius)) = circle else {
                        return false;
                    };
                    let circle_index = sketch.entities.len();
                    let constraints = selected.iter().map(|&line| Constraint::Tangent {
                        line: EntityRef(line),
                        circle: EntityRef(circle_index),
                    });
                    let changed = document.add_sketch_entities_with_constraints(
                        id,
                        [SketchEntity::Circle { center, radius }],
                        constraints,
                    );
                    if changed {
                        cx.notify();
                    }
                    changed
                });
                if !created {
                    self.gizmo_readout = Some((
                        "请选择所需数量的非平行直线".into(),
                        pointer + Vec2::new(14.0, -22.0),
                    ));
                }
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
            if matches!(sketch_tool, Some(ToolId::Extend | ToolId::Break))
                && let Some(SelItem::SketchEntity(id, entity)) = self.sketch_entity_at(pointer, cx)
                && let Some(local) = self.sketch_local_at(pointer)
            {
                let changed = self.document.update(cx, |document, cx| {
                    let changed = if sketch_tool == Some(ToolId::Extend) {
                        document.extend_sketch_entity(id, entity, local)
                    } else {
                        document.break_sketch_entity(id, entity, local)
                    };
                    if changed {
                        cx.notify();
                    }
                    changed
                });
                self.gizmo_readout = (!changed).then(|| {
                    (
                        if sketch_tool == Some(ToolId::Extend) {
                            "未找到可延伸到的交点"
                        } else {
                            "该位置无法打断"
                        }
                        .to_owned(),
                        pointer + Vec2::new(14.0, -22.0),
                    )
                });
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
            if sketch_tool == Some(ToolId::Trim)
                && let Some(SelItem::SketchEntity(id, entity)) = self.sketch_entity_at(pointer, cx)
                && let Some(local) = self.sketch_local_at(pointer)
            {
                self.document.update(cx, |document, cx| {
                    if document.trim_sketch_entity(id, entity, local) {
                        cx.notify();
                    }
                });
                if let Some(interaction) = &mut self.sketch_interaction {
                    interaction.trim_preview = None;
                }
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
            if sketch_tool == Some(ToolId::SketchFillet)
                && let Some(SelItem::SketchEntity(_, entity)) = self.sketch_entity_at(pointer, cx)
            {
                let local = self.sketch_local_at(pointer);
                let interaction = self.sketch_interaction.as_mut().expect("tool checked");
                if let Some(first) = interaction.fillet_first
                    && first != entity
                {
                    interaction.fillet_pair = Some((first, entity));
                    interaction.anchor = local;
                    interaction.cursor = interaction.anchor;
                    interaction.edit_distance = 5.0;
                } else {
                    interaction.fillet_first = Some(entity);
                }
                self.changed(window, cx);
                return;
            }
            if sketch_tool == Some(ToolId::SketchOffset)
                && let Some(SelItem::Profile(id, profile)) = self.profile_at(pointer, cx)
                && let Some(local) = self.sketch_local_at(pointer)
            {
                let interaction = self.sketch_interaction.as_mut().expect("tool checked");
                debug_assert_eq!(interaction.id, id);
                interaction.edit_profile = Some(profile);
                interaction.anchor = Some(local);
                interaction.cursor = Some(local);
                interaction.edit_distance = 0.0;
                self.changed(window, cx);
                return;
            }
            if let Some(SelItem::SketchEntity(id, entity)) = self.sketch_entity_at(pointer, cx) {
                let shift = event.modifiers.shift;
                let start = self
                    .document
                    .read(cx)
                    .sketches
                    .iter()
                    .find(|sketch| sketch.id == id)
                    .cloned();
                self.document.update(cx, |document, cx| {
                    document
                        .selection
                        .apply(SelItem::SketchEntity(id, entity), shift);
                    cx.notify();
                });
                if !shift
                    && let (Some(start), Some(pointer_start)) =
                        (start, self.sketch_local_at(pointer))
                {
                    self.sketch_entity_drag = Some(SketchEntityDrag {
                        id,
                        entity,
                        start,
                        pointer_start,
                        moved: false,
                    });
                }
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
            if self.sketch_interaction.is_some() && {
                let arm_drag = self.sketch_interaction.as_ref().is_some_and(|interaction| {
                    matches!(interaction.tool, ToolId::Rectangle | ToolId::Circle)
                        && interaction.anchor.is_none()
                });
                if arm_drag {
                    self.sketch_press = Some(pointer);
                }
                self.sketch_mouse_down(pointer, event.click_count >= 2, cx)
            } {
                self.changed(window, cx);
                return;
            }
            let filter = self.document.read(cx).selection.filter;
            let direct_item = self
                .construction_plane_at(pointer, cx)
                // Closed profiles of the active sketch stay clickable outside
                // drawing tools (the Shapr3D pull-a-profile flow).
                .or_else(|| self.profile_at(pointer, cx));
            let shift = event.modifiers.shift;
            let all_candidates = direct_item
                .is_none()
                .then(|| self.pick_candidates(pointer, event.click_count >= 2, filter))
                .unwrap_or_default();
            let (popup_candidates, ambiguous) =
                self.click_candidates(pointer, event.click_count >= 2, filter);
            if direct_item.is_none()
                && !event.modifiers.platform
                && (self.select_through || ambiguous)
                && !all_candidates.is_empty()
            {
                self.pick_popup = Some(PickPopup {
                    candidates: if self.select_through {
                        all_candidates.clone()
                    } else {
                        popup_candidates
                    },
                    position: pointer + Vec2::new(12.0, 12.0),
                    shift,
                });
                self.hovered = all_candidates.first().map(|candidate| candidate.item);
                self.changed(window, cx);
                return;
            }
            let item = direct_item.or_else(|| {
                resolve_selection_candidate(&all_candidates, event.modifiers.platform)
                    .map(|candidate| candidate.item)
            });
            let geometry_hit = item.is_some()
                || self
                    .construction_plane_at(pointer, cx)
                    .or_else(|| {
                        self.item_at(pointer, event.click_count >= 2, SelectionFilter::Auto)
                    })
                    .is_some();
            if !geometry_hit && self.sketch_interaction.is_none() {
                self.marquee = Some(MarqueeInteraction {
                    start: pointer,
                    current: pointer,
                    shift,
                    active: false,
                });
                self.hovered = None;
                self.changed(window, cx);
                return;
            }
            self.document.update(cx, |document, cx| {
                if let Some(item) = item {
                    document.selection.apply(item, shift);
                } else if !shift {
                    document.selection.clear();
                }
                cx.notify();
            });
            self.changed(window, cx);
            return;
        }
        let gesture = GestureKind::MouseDrag(event.button);
        self.dragging = resolve(self.nav_preset, gesture, &event.modifiers);
        if self.dragging.is_some() {
            self.changed(window, cx);
        }
    }

    fn mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pointer = Self::pointer(event.position, window.scale_factor());
        let delta = pointer - self.last_pointer;
        self.last_pointer = pointer;
        if self.numeric_input.is_some() {
            return;
        }
        if self.pick_popup.is_some() {
            return;
        }
        if let Some(mut drag) = self.joint_drive {
            let pixels = f64::from(pointer.x - drag.start_x);
            drag.current_value = drag.start_value
                + match drag.kind {
                    JointKind::Revolute => pixels * 0.01,
                    JointKind::Slider | JointKind::Cylindrical => {
                        pixels * f64::from(self.camera.distance) / 500.0
                    }
                    JointKind::Fixed | JointKind::Ball => 0.0,
                };
            self.joint_drive = Some(drag);
            self.document.update(cx, |document, cx| {
                if let Some(joint) = document.joints.iter_mut().find(|joint| joint.id == drag.id) {
                    joint.value = drag.current_value;
                }
                document.solve_assembly();
                cx.notify();
            });
            self.gizmo_readout = Some((
                if drag.kind == JointKind::Revolute {
                    format!("{:.1}°", drag.current_value.to_degrees())
                } else {
                    format!("{:.2} mm", drag.current_value)
                },
                pointer + Vec2::new(14.0, -22.0),
            ));
            self.changed(window, cx);
            return;
        }
        if let Some(marquee) = &mut self.marquee {
            marquee.current = pointer;
            marquee.active |= pointer.distance(marquee.start) > 4.0 * self.device_scale.max(1.0);
            self.changed(window, cx);
            return;
        }
        let cube_size = self.cube_rect().size;
        if let Some(interaction) = &mut self.cube_interaction {
            if pointer.distance(interaction.press_pointer) > 3.0 * self.device_scale.max(1.0) {
                interaction.dragged = true;
            }
            if interaction.dragged {
                let orbit_scale = std::f32::consts::PI / cube_size / 0.006;
                self.camera.orbit(delta * orbit_scale);
            }
            self.hovered_cube = self.cube_region_at(pointer);
            self.changed(window, cx);
            return;
        }
        if self.cube_rect().contains(pointer) {
            let hovered = self.cube_region_at(pointer);
            if hovered != self.hovered_cube {
                self.hovered_cube = hovered;
                self.changed(window, cx);
            }
            return;
        }
        if self.hovered_cube.take().is_some() {
            self.changed(window, cx);
        }
        if let Some(interaction) = &mut self.m6_interaction {
            match interaction {
                M6Interaction::Scale {
                    factor,
                    drag: Some((start, initial)),
                    ..
                } => {
                    *factor = (*initial * f64::from(((pointer.x - *start) / 180.0).exp()))
                        .clamp(0.01, 100.0);
                    self.gizmo_readout =
                        Some((format!("×{factor:.2}"), pointer + Vec2::new(14.0, -22.0)));
                    self.changed(window, cx);
                    return;
                }
                M6Interaction::Split {
                    y,
                    drag: Some((start, initial)),
                    ..
                } => {
                    *y = *initial
                        + f64::from(pointer.x - *start) * f64::from(self.camera.distance) / 500.0;
                    self.section_offset = Some(*y as f32);
                    self.gizmo_readout = Some((
                        format!("Y {y:.1} · Enter"),
                        pointer + Vec2::new(14.0, -22.0),
                    ));
                    self.changed(window, cx);
                    return;
                }
                M6Interaction::ConstructionPlane {
                    distance,
                    drag: Some((start, initial)),
                    ..
                } => {
                    *distance = *initial
                        + f64::from(pointer.x - *start) * f64::from(self.camera.distance) / 500.0;
                    self.gizmo_readout = Some((
                        format!("offset {distance:.1} · Enter"),
                        pointer + Vec2::new(14.0, -22.0),
                    ));
                    self.changed(window, cx);
                    return;
                }
                _ => {}
            }
        }
        if self.section_drag.is_some() {
            self.update_section_drag(pointer);
            self.changed(window, cx);
            return;
        }
        if self.gizmo_drag.is_some() {
            self.update_gizmo_drag(pointer, event.modifiers.shift);
            self.changed(window, cx);
            return;
        }
        if let Some(mut drag) = self.sketch_entity_drag.take() {
            if let Some(local) = self.sketch_local_at(pointer) {
                let delta = local - drag.pointer_start;
                drag.moved |= delta.length_squared() > 1.0e-10;
                self.document.update(cx, |document, cx| {
                    if !document.preview_sketch_drag(drag.id, drag.entity, delta, &drag.start) {
                        eprintln!("sketch drag solve did not converge");
                    }
                    cx.notify();
                });
                self.sync_sketch_gpu(cx);
            }
            self.sketch_entity_drag = Some(drag);
            self.changed(window, cx);
            return;
        }
        if self.update_sketch_pattern(pointer) {
            self.changed(window, cx);
            return;
        }
        if self.update_hole_drag(pointer) {
            self.changed(window, cx);
            return;
        }
        if self.update_feature_drag(pointer) {
            self.changed(window, cx);
            return;
        }
        if self.extrude_drag.is_some()
            || self.profile_extrude_drag.is_some()
            || self.open_chain_extrude_drag.is_some()
        {
            self.update_extrude_drag(pointer, cx);
            self.changed(window, cx);
            return;
        }
        if self.dressup_drag.is_some() {
            self.update_dressup_drag(pointer, cx);
            self.changed(window, cx);
            return;
        }
        if self.shell_drag.is_some() {
            self.update_shell_drag(pointer, cx);
            self.changed(window, cx);
            return;
        }
        if self.thicken_drag.is_some() {
            self.update_thicken_drag(pointer, cx);
            self.changed(window, cx);
            return;
        }
        if self.sketch_interaction.as_ref().is_some_and(|interaction| {
            interaction.fillet_pair.is_some() || interaction.edit_profile.is_some()
        }) && let Some(local) = self.sketch_local_at(pointer)
        {
            let (id, profile, anchor) = {
                let interaction = self.sketch_interaction.as_ref().expect("checked above");
                (interaction.id, interaction.edit_profile, interaction.anchor)
            };
            let mut distance = anchor.map_or(0.0, |anchor| anchor.distance(local));
            if let Some(profile) = profile {
                let inside = self
                    .document
                    .read(cx)
                    .sketches
                    .iter()
                    .find(|sketch| sketch.id == id)
                    .and_then(|sketch| {
                        sketch
                            .profiles()
                            .get(profile)
                            .map(|p| profile_contains(sketch, p, local))
                    })
                    .unwrap_or(false);
                if inside {
                    distance = -distance;
                }
            }
            let interaction = self.sketch_interaction.as_mut().expect("checked above");
            interaction.cursor = Some(local);
            interaction.edit_distance = distance;
            self.gizmo_readout = Some((
                if interaction.fillet_pair.is_some() {
                    format!("r {:.1}", distance.abs())
                } else {
                    format!("{distance:.1}")
                },
                pointer + Vec2::new(14.0, -22.0),
            ));
            self.changed(window, cx);
            return;
        }
        if self.sketch_interaction.is_some() && self.dragging.is_none() {
            if let Some((point, snapped, _)) = self.snapped_sketch_point(pointer, cx) {
                let has_anchor = self
                    .sketch_interaction
                    .as_ref()
                    .is_some_and(|interaction| interaction.anchor.is_some());
                if has_anchor {
                    let interaction = self.sketch_interaction.as_mut().expect("checked above");
                    interaction.cursor = Some(point);
                    interaction.hv_snapped = snapped;
                    if interaction.tool == ToolId::Circle {
                        let radius = interaction.anchor.expect("anchor checked").distance(point);
                        self.gizmo_readout =
                            Some((format!("r {radius:.1}"), pointer + Vec2::new(14.0, -22.0)));
                    } else if interaction.tool == ToolId::Polygon {
                        self.gizmo_readout = Some((
                            format!("[−] {} 边 [+]", interaction.polygon_sides),
                            pointer + Vec2::new(14.0, -22.0),
                        ));
                    } else if matches!(interaction.tool, ToolId::Slot | ToolId::RoundedRectangle)
                        && let Some(second) = interaction.arc_end
                    {
                        let radius = if interaction.tool == ToolId::Slot {
                            let axis = (second - interaction.anchor.expect("anchor checked"))
                                .normalize_or_zero();
                            (point - second).perp_dot(axis).abs()
                        } else {
                            point.distance(second)
                        };
                        self.gizmo_readout =
                            Some((format!("r {radius:.1}"), pointer + Vec2::new(14.0, -22.0)));
                    } else if matches!(interaction.tool, ToolId::Ellipse | ToolId::EllipseArc) {
                        let center = interaction.anchor.expect("anchor checked");
                        let (major_radius, minor_radius) =
                            if let Some(major_end) = interaction.arc_end {
                                let major = major_end - center;
                                let perpendicular =
                                    glam::DVec2::new(-major.y, major.x).normalize_or_zero();
                                (
                                    major.length(),
                                    (point - center)
                                        .dot(perpendicular)
                                        .abs()
                                        .min(major.length()),
                                )
                            } else {
                                (center.distance(point), 0.0)
                            };
                        self.gizmo_readout = Some((
                            format!("a {major_radius:.1}  b {minor_radius:.1}"),
                            pointer + Vec2::new(14.0, -22.0),
                        ));
                    } else if interaction.tool == ToolId::Arc
                        && let Some(end) = interaction.arc_end
                        && let Some((_, radius)) = arc_center_radius(
                            interaction.anchor.expect("anchor checked"),
                            point,
                            end,
                        )
                    {
                        self.gizmo_readout =
                            Some((format!("r {radius:.1}"), pointer + Vec2::new(14.0, -22.0)));
                    }
                    self.hovered = None;
                    self.sync_sketch_gpu(cx);
                } else {
                    self.hovered = self
                        .sketch_entity_at(pointer, cx)
                        .or_else(|| self.profile_at(pointer, cx));
                    let trim_preview = if let Some(SelItem::SketchEntity(id, entity)) = self.hovered
                        && self
                            .sketch_interaction
                            .as_ref()
                            .is_some_and(|interaction| interaction.tool == ToolId::Trim)
                    {
                        self.document
                            .read(cx)
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == id)
                            .and_then(|sketch| sketch.trim_subsegment(entity, point))
                    } else {
                        None
                    };
                    if trim_preview.is_none()
                        && let Some(SelItem::SketchEntity(id, entity)) = self.hovered
                        && self
                            .sketch_interaction
                            .as_ref()
                            .is_some_and(|interaction| interaction.tool == ToolId::Trim)
                        && self
                            .document
                            .read(cx)
                            .sketches
                            .iter()
                            .find(|sketch| sketch.id == id)
                            .and_then(|sketch| sketch.entities.get(entity))
                            .is_some_and(|item| matches!(item.geo, SketchEntity::EllipseArc { .. }))
                    {
                        self.gizmo_readout = Some((
                            "椭圆弧暂不支持修剪".into(),
                            pointer + Vec2::new(14.0, -22.0),
                        ));
                    }
                    if let Some(interaction) = &mut self.sketch_interaction {
                        interaction.trim_preview = trim_preview;
                    }
                    self.sync_sketch_gpu(cx);
                }
                self.changed(window, cx);
                return;
            }
        }
        match self.dragging {
            Some(NavAction::Orbit) => self.camera.orbit(delta),
            Some(NavAction::Pan) => self.camera.pan(delta),
            _ => {
                let section_hover = self.section_arrow_state().is_some_and(|arrow| {
                    hit_test_axis(
                        self.camera.unproject_ray(pointer),
                        arrow.origin,
                        arrow.normal,
                        arrow.scale,
                    )
                });
                if section_hover != self.hovered_section_arrow {
                    self.hovered_section_arrow = section_hover;
                    self.changed(window, cx);
                }
                if section_hover {
                    return;
                }
                let extrude_hover = self.tool_arrow_state(cx).is_some_and(|arrow| {
                    hit_test_axis(
                        self.camera.unproject_ray(pointer),
                        arrow.origin,
                        arrow.normal,
                        arrow.scale,
                    )
                });
                if extrude_hover != self.hovered_extrude_arrow {
                    self.hovered_extrude_arrow = extrude_hover;
                    self.changed(window, cx);
                }
                if extrude_hover {
                    return;
                }
                let gizmo_hover = self.selection_pivot(cx).and_then(|pivot| {
                    hit_test(
                        self.camera.unproject_ray(pointer),
                        pivot,
                        self.gizmo_scale(pivot),
                    )
                });
                if gizmo_hover != self.hovered_gizmo {
                    self.hovered_gizmo = gizmo_hover;
                    self.changed(window, cx);
                }
                if gizmo_hover.is_some() {
                    return;
                }
                let filter = self.document.read(cx).selection.filter;
                let hovered = self
                    .construction_plane_at(pointer, cx)
                    .or_else(|| self.item_at(pointer, false, filter));
                if hovered != self.hovered {
                    self.hovered = hovered;
                    self.changed(window, cx);
                }
                return;
            }
        }
        self.changed(window, cx);
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.numeric_input.is_some() {
            cx.stop_propagation();
            return;
        }
        if event.button == MouseButton::Left
            && let Some(drag) = self.joint_drive.take()
        {
            self.document.update(cx, |document, cx| {
                if let Some(joint) = document.joints.iter_mut().find(|joint| joint.id == drag.id) {
                    joint.value = drag.start_value;
                    joint.value2 = drag.start_value2;
                }
                document.set_joint_value(drag.id, drag.current_value, drag.start_value2);
                cx.notify();
            });
            self.gizmo_readout = None;
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left && self.section_drag.take().is_some() {
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left {
            let finish_scale = matches!(
                self.m6_interaction,
                Some(M6Interaction::Scale { drag: Some(_), .. })
            );
            let release_split = matches!(
                self.m6_interaction,
                Some(M6Interaction::Split { drag: Some(_), .. })
            );
            let release_plane = matches!(
                self.m6_interaction,
                Some(M6Interaction::ConstructionPlane { drag: Some(_), .. })
            );
            if finish_scale {
                self.commit_m6(cx);
                self.changed(window, cx);
                return;
            }
            if release_split {
                if let Some(M6Interaction::Split { drag, .. }) = &mut self.m6_interaction {
                    *drag = None;
                }
                self.changed(window, cx);
                return;
            }
            if release_plane {
                let pointer = Self::pointer(event.position, window.scale_factor());
                let clicked = matches!(
                    self.m6_interaction,
                    Some(M6Interaction::ConstructionPlane {
                        drag: Some((start, _)),
                        ..
                    }) if (pointer.x - start).abs() < 3.0 * self.device_scale.max(1.0)
                );
                if clicked {
                    self.commit_m6(cx);
                    self.changed(window, cx);
                    return;
                }
                if let Some(M6Interaction::ConstructionPlane { drag, .. }) =
                    &mut self.m6_interaction
                {
                    *drag = None;
                }
                self.changed(window, cx);
                return;
            }
        }
        if event.button == MouseButton::Left && self.marquee.is_some() {
            let pointer = Self::pointer(event.position, window.scale_factor());
            if let Some(marquee) = &mut self.marquee {
                marquee.current = pointer;
            }
            self.finish_marquee(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left
            && let Some(interaction) = self.cube_interaction.take()
        {
            let pointer = Self::pointer(event.position, window.scale_factor());
            if !interaction.dragged
                && let Some(region) = self.cube_region_at(pointer).or(interaction.pressed_region)
            {
                self.go_to_cube_region(region, window, cx);
            } else {
                self.changed(window, cx);
            }
            return;
        }
        if event.button == MouseButton::Left {
            let edit = self.sketch_interaction.as_ref().and_then(|interaction| {
                if let Some((first, second)) = interaction.fillet_pair {
                    Some((
                        interaction.id,
                        Some((first, second)),
                        None,
                        interaction.edit_distance.abs(),
                    ))
                } else {
                    interaction.edit_profile.map(|profile| {
                        (
                            interaction.id,
                            None,
                            Some(profile),
                            interaction.edit_distance,
                        )
                    })
                }
            });
            if let Some((id, fillet, profile, distance)) = edit {
                self.document.update(cx, |document, cx| {
                    let changed = if let Some((first, second)) = fillet {
                        document.fillet_sketch_lines(id, first, second, distance)
                    } else {
                        document.offset_sketch_profile(
                            id,
                            profile.expect("offset profile"),
                            distance,
                        )
                    };
                    if changed {
                        cx.notify();
                    }
                });
                if let Some(interaction) = &mut self.sketch_interaction {
                    interaction.anchor = None;
                    interaction.cursor = None;
                    interaction.fillet_first = None;
                    interaction.fillet_pair = None;
                    interaction.edit_profile = None;
                    interaction.edit_distance = 0.0;
                }
                self.gizmo_readout = None;
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
        }
        if event.button == MouseButton::Left
            && let Some(press) = self.sketch_press.take()
        {
            let pointer = Self::pointer(event.position, window.scale_factor());
            if pointer.distance(press) > 3.0 * self.device_scale.max(1.0) {
                self.sketch_mouse_down(pointer, false, cx);
                self.changed(window, cx);
                return;
            }
        }
        if event.button == MouseButton::Left
            && let Some(drag) = self.sketch_entity_drag.take()
        {
            self.document.update(cx, |document, cx| {
                if !document.finish_sketch_drag(drag.id, drag.start, drag.moved) {
                    eprintln!("final sketch drag solve did not converge; drag cancelled");
                }
                cx.notify();
            });
            self.sync_sketch_gpu(cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left && self.dressup_drag.is_some() {
            self.finish_dressup_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left && self.shell_drag.is_some() {
            self.finish_shell_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left && self.thicken_drag.is_some() {
            self.finish_thicken_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left
            && self
                .hole_interaction
                .as_ref()
                .is_some_and(|interaction| interaction.start_x.is_some())
        {
            self.finish_hole_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left
            && self
                .sketch_pattern_interaction
                .as_ref()
                .is_some_and(|interaction| interaction.anchor.is_some())
        {
            self.finish_sketch_pattern(cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left
            && (self
                .revolve_interaction
                .as_ref()
                .is_some_and(|interaction| interaction.start_x.is_some())
                || self
                    .pattern_interaction
                    .as_ref()
                    .is_some_and(|interaction| interaction.start_x.is_some())
                || self
                    .draft_interaction
                    .as_ref()
                    .is_some_and(|interaction| interaction.start_x.is_some()))
        {
            self.finish_feature_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left
            && (self.extrude_drag.is_some()
                || self.profile_extrude_drag.is_some()
                || self.open_chain_extrude_drag.is_some())
        {
            self.finish_extrude_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        if event.button == MouseButton::Left && self.gizmo_drag.is_some() {
            self.finish_gizmo_drag(true, cx);
            self.changed(window, cx);
            return;
        }
        self.dragging = None;
        self.changed(window, cx);
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pointer = Self::pointer(event.position, window.scale_factor());
        if self.cube_rect().contains(pointer) {
            return;
        }
        let action = resolve(
            self.nav_preset,
            scroll_gesture(&event.delta),
            &event.modifiers,
        );
        match (&event.delta, action) {
            (ScrollDelta::Pixels(delta), Some(NavAction::Orbit)) => {
                self.camera
                    .orbit(Vec2::new(f32::from(delta.x), f32::from(delta.y)));
            }
            (ScrollDelta::Pixels(delta), Some(NavAction::Pan)) => {
                self.camera
                    .pan(Vec2::new(f32::from(delta.x), f32::from(delta.y)));
            }
            (ScrollDelta::Pixels(delta), Some(NavAction::Zoom)) => {
                let point = self.zoom_anchor(pointer, cx);
                self.camera
                    .zoom_toward_point(point, f32::from(delta.y) * 0.008);
            }
            (ScrollDelta::Lines(delta), Some(NavAction::Zoom)) => {
                let point = self.zoom_anchor(pointer, cx);
                self.camera.zoom_toward_point(point, delta.y * 0.12);
            }
            _ => return,
        }
        self.changed(window, cx);
    }

    fn pinch(&mut self, event: &PinchEvent, window: &mut Window, cx: &mut Context<Self>) {
        let cursor = Self::pointer(event.position, window.scale_factor());
        if self.cube_rect().contains(cursor) {
            return;
        }
        if resolve(self.nav_preset, GestureKind::Pinch, &event.modifiers) != Some(NavAction::Zoom) {
            return;
        }
        let point = self.zoom_anchor(cursor, cx);
        self.camera
            .zoom_toward_point(point, -(1.0 + event.delta).max(0.05).ln());
        self.changed(window, cx);
    }

    fn supports_numeric_input(&self) -> bool {
        self.joint_drive.is_some()
            || self.helix_interaction.is_some()
            || self.thread_interaction.is_some()
            || self.revolve_interaction.is_some()
            || self
                .hole_interaction
                .as_ref()
                .is_some_and(|interaction| interaction.at.is_some())
            || self.draft_interaction.is_some()
            || self
                .pattern_interaction
                .as_ref()
                .is_some_and(|interaction| interaction.mode == PatternMode::Linear)
            || self.extrude_drag.is_some()
            || self.profile_extrude_drag.is_some()
            || self.open_chain_extrude_drag.is_some()
            || self.dressup_drag.is_some()
            || self.shell_drag.is_some()
            || self.thicken_drag.is_some()
            || self
                .gizmo_drag
                .as_ref()
                .is_some_and(|drag| !matches!(drag.anchor, DragAnchor::Center(_)))
            || self.sketch_interaction.as_ref().is_some_and(|interaction| {
                (interaction.tool == ToolId::Circle && interaction.anchor.is_some())
                    || (interaction.tool == ToolId::Polygon && interaction.anchor.is_some())
                    || (matches!(interaction.tool, ToolId::Slot | ToolId::RoundedRectangle)
                        && interaction.arc_end.is_some())
                    || interaction.fillet_pair.is_some()
                    || interaction.edit_profile.is_some()
            })
    }

    fn numeric_seed(event: &KeyDownEvent) -> Option<char> {
        if event.keystroke.modifiers.platform || event.keystroke.modifiers.control {
            return None;
        }
        let mut characters = event.keystroke.key_char.as_deref()?.chars();
        let character = characters.next()?;
        (characters.next().is_none() && character.is_ascii_digit()).then_some(character)
    }

    fn numeric_input_uses_units(&self) -> bool {
        if self
            .joint_drive
            .is_some_and(|drive| drive.kind == JointKind::Revolute)
        {
            return false;
        }
        if self
            .helix_interaction
            .as_ref()
            .is_some_and(|interaction| interaction.phase == HelixPhase::Turns)
            || self.revolve_interaction.is_some()
            || self.draft_interaction.is_some()
            || self
                .gizmo_drag
                .as_ref()
                .is_some_and(|drag| matches!(drag.anchor, DragAnchor::Ring(_)))
            || self.sketch_interaction.as_ref().is_some_and(|interaction| {
                interaction.tool == ToolId::Polygon && interaction.anchor.is_some()
            })
        {
            return false;
        }
        true
    }

    fn numeric_drag_transition(
        supports_input: bool,
        input_open: bool,
        event: &KeyDownEvent,
    ) -> NumericDragTransition {
        if supports_input && !input_open {
            Self::numeric_seed(event)
                .map(NumericDragTransition::Freeze)
                .unwrap_or(NumericDragTransition::Ignore)
        } else {
            NumericDragTransition::Ignore
        }
    }

    fn begin_numeric_input(&mut self, seed: char, window: &mut Window, cx: &mut Context<Self>) {
        let position = self
            .gizmo_readout
            .as_ref()
            .map(|(_, position)| *position)
            .unwrap_or(self.last_pointer + Vec2::new(14.0, -22.0));
        let theme = self.theme.clone();
        let variables = self.numeric_variables(cx);
        let units = self.units;
        let uses_units = self.numeric_input_uses_units();
        let input = cx.new(|cx| {
            let input =
                NumericInput::new_with_variables(seed.to_string(), "", theme, variables, cx);
            if uses_units {
                input.with_units(units)
            } else {
                input
            }
        });
        let subscription = cx.subscribe_in(&input, window, |viewport, _, event, window, cx| {
            viewport.handle_numeric_input(event.clone(), window, cx);
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.numeric_input = Some((input, position));
        self.numeric_input_subscription = Some(subscription);
        self.changed(window, cx);
    }

    fn numeric_variables(&self, cx: &Context<Self>) -> HashMap<String, f64> {
        self.document
            .read(cx)
            .variables
            .iter()
            .map(|variable| (variable.name.clone(), variable.value))
            .collect()
    }

    fn dimension_points(sketch: &Sketch, entity: usize) -> Vec<(PointRef, glam::DVec2)> {
        let point = |point, at| (PointRef { entity, point }, at);
        match &sketch.entities[entity].geo {
            SketchEntity::Line { a, b } => vec![point(0, *a), point(1, *b)],
            SketchEntity::Arc { start, end, .. } => vec![point(0, *start), point(1, *end)],
            SketchEntity::Spline { points } if points.len() >= 2 => vec![
                point(0, points[0]),
                point(1, *points.last().expect("spline endpoint")),
            ],
            SketchEntity::CvSpline { control, .. } if control.len() >= 2 => vec![
                point(0, control[0]),
                point(1, *control.last().expect("CV spline endpoint")),
            ],
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => vec![
                point(
                    0,
                    ellipse_point(*center, *major, *minor_ratio, *start_angle),
                ),
                point(1, ellipse_point(*center, *major, *minor_ratio, *end_angle)),
            ],
            SketchEntity::Point { at } => vec![point(0, *at)],
            _ => Vec::new(),
        }
    }

    fn dimension_measurement(
        sketch: &Sketch,
        target: SketchDimensionTarget,
    ) -> Option<(f64, glam::DVec2, &'static str)> {
        match target {
            SketchDimensionTarget::Length(entity) => match sketch.entities[entity.0].geo {
                SketchEntity::Line { a, b } => Some((a.distance(b), (a + b) * 0.5, "长度")),
                _ => None,
            },
            SketchDimensionTarget::Radius(entity) | SketchDimensionTarget::Diameter(entity) => {
                let (center, radius) = match sketch.entities[entity.0].geo {
                    SketchEntity::Circle { center, radius } => (center, radius),
                    SketchEntity::Arc { start, end, mid } => arc_center_radius(start, mid, end)?,
                    _ => return None,
                };
                if matches!(target, SketchDimensionTarget::Diameter(_)) {
                    Some((radius * 2.0, center + glam::DVec2::X * radius, "直径"))
                } else {
                    Some((radius, center + glam::DVec2::X * radius, "半径"))
                }
            }
            SketchDimensionTarget::Distance { a, b }
            | SketchDimensionTarget::HDistance { a, b, .. }
            | SketchDimensionTarget::VDistance { a, b, .. } => {
                let point = |reference: PointRef| {
                    Self::dimension_points(sketch, reference.entity)
                        .into_iter()
                        .find(|(candidate, _)| *candidate == reference)
                        .map(|(_, point)| point)
                };
                let (a_point, b_point) = (point(a)?, point(b)?);
                let (value, label) = if matches!(target, SketchDimensionTarget::Distance { .. }) {
                    (a_point.distance(b_point), "距离")
                } else if matches!(target, SketchDimensionTarget::HDistance { .. }) {
                    ((a_point.x - b_point.x).abs(), "水平距离")
                } else {
                    ((a_point.y - b_point.y).abs(), "垂直距离")
                };
                Some((value, (a_point + b_point) * 0.5, label))
            }
            SketchDimensionTarget::Angle { a, b } => {
                let direction = |entity: EntityRef| match sketch.entities[entity.0].geo {
                    SketchEntity::Line { a, b } => Some(b - a),
                    _ => None,
                };
                let (a_direction, b_direction) = (direction(a)?, direction(b)?);
                let angle = b_direction.y.atan2(b_direction.x) - a_direction.y.atan2(a_direction.x);
                let anchor = match (&sketch.entities[a.0].geo, &sketch.entities[b.0].geo) {
                    (SketchEntity::Line { a: a0, b: a1 }, SketchEntity::Line { a: b0, b: b1 }) => {
                        (*a0 + *a1 + *b0 + *b1) * 0.25
                    }
                    _ => glam::DVec2::ZERO,
                };
                Some((
                    angle.sin().atan2(angle.cos()).to_degrees().abs(),
                    anchor,
                    "角度",
                ))
            }
        }
    }

    fn dimension_readouts(&self, cx: &Context<Self>) -> Vec<DimensionReadout> {
        let document = self.document.read(cx);
        let Some(id) = document.active_sketch else {
            return Vec::new();
        };
        let Some(sketch) = document.sketches.iter().find(|sketch| sketch.id == id) else {
            return Vec::new();
        };
        let selected: Vec<_> = document
            .selection
            .items
            .iter()
            .filter_map(|item| match item {
                SelItem::SketchEntity(selected_id, entity) if *selected_id == id => Some(*entity),
                _ => None,
            })
            .collect();
        let mut targets = Vec::new();
        match selected.as_slice() {
            [entity] => match sketch.entities[*entity].geo {
                SketchEntity::Line { .. } => {
                    targets.push(SketchDimensionTarget::Length(EntityRef(*entity)))
                }
                SketchEntity::Circle { .. } => targets.extend([
                    SketchDimensionTarget::Diameter(EntityRef(*entity)),
                    SketchDimensionTarget::Radius(EntityRef(*entity)),
                ]),
                SketchEntity::Arc { .. } => {
                    targets.push(SketchDimensionTarget::Radius(EntityRef(*entity)))
                }
                _ => {}
            },
            [first, second] => {
                let a = Self::dimension_points(sketch, *first);
                let b = Self::dimension_points(sketch, *second);
                if let Some((a, b, delta)) = a
                    .iter()
                    .flat_map(|a| b.iter().map(move |b| (a.0, b.0, a.1 - b.1)))
                    .min_by(|left, right| {
                        left.2.length_squared().total_cmp(&right.2.length_squared())
                    })
                {
                    targets.push(SketchDimensionTarget::HDistance {
                        a,
                        b,
                        sign: if delta.x < 0.0 { -1.0 } else { 1.0 },
                    });
                    targets.push(SketchDimensionTarget::VDistance {
                        a,
                        b,
                        sign: if delta.y < 0.0 { -1.0 } else { 1.0 },
                    });
                }
                if matches!(sketch.entities[*first].geo, SketchEntity::Line { .. })
                    && matches!(sketch.entities[*second].geo, SketchEntity::Line { .. })
                {
                    targets.push(SketchDimensionTarget::Angle {
                        a: EntityRef(*first),
                        b: EntityRef(*second),
                    });
                }
            }
            _ => {}
        }
        let mut readouts = Vec::new();
        for (index, target) in targets.into_iter().enumerate() {
            let Some((value, anchor, name)) = Self::dimension_measurement(sketch, target) else {
                continue;
            };
            let (reference, expression) = sketch
                .constraints
                .iter()
                .find_map(|constraint| match (constraint, target) {
                    (
                        Constraint::Length {
                            line,
                            reference,
                            expr,
                            ..
                        },
                        SketchDimensionTarget::Length(target),
                    ) if *line == target => Some((*reference, expr.clone())),
                    (
                        Constraint::Radius {
                            circle,
                            reference,
                            expr,
                            ..
                        }
                        | Constraint::Diameter {
                            circle,
                            reference,
                            expr,
                            ..
                        },
                        SketchDimensionTarget::Radius(target)
                        | SketchDimensionTarget::Diameter(target),
                    ) if *circle == target => Some((*reference, expr.clone())),
                    (
                        Constraint::Distance {
                            a,
                            b,
                            reference,
                            expr,
                            ..
                        },
                        SketchDimensionTarget::Distance { a: ta, b: tb },
                    ) if *a == ta && *b == tb => Some((*reference, expr.clone())),
                    (
                        Constraint::HDistance {
                            a,
                            b,
                            reference,
                            expr,
                            ..
                        },
                        SketchDimensionTarget::HDistance { a: ta, b: tb, .. },
                    ) if *a == ta && *b == tb => Some((*reference, expr.clone())),
                    (
                        Constraint::VDistance {
                            a,
                            b,
                            reference,
                            expr,
                            ..
                        },
                        SketchDimensionTarget::VDistance { a: ta, b: tb, .. },
                    ) if *a == ta && *b == tb => Some((*reference, expr.clone())),
                    (
                        Constraint::Angle {
                            a,
                            b,
                            reference,
                            expr,
                            ..
                        },
                        SketchDimensionTarget::Angle { a: ta, b: tb },
                    ) if *a == ta && *b == tb => Some((*reference, expr.clone())),
                    _ => None,
                })
                .unwrap_or((false, None));
            readouts.push(DimensionReadout {
                id,
                target,
                label: format!(
                    "{}{name} {}",
                    if reference { "参考 " } else { "" },
                    self.dimension_display(target, value)
                ),
                position: self.camera.project(sketch.plane.to_world(anchor).as_vec3())
                    + Vec2::new(10.0, -18.0 + index as f32 * 27.0),
                value,
                reference,
                expression,
            });
        }
        readouts
    }

    fn reference_dimension_readouts(&self, cx: &Context<Self>) -> Vec<DimensionReadout> {
        let document = self.document.read(cx);
        let Some(id) = document.active_sketch else {
            return Vec::new();
        };
        let Some(sketch) = document.sketches.iter().find(|sketch| sketch.id == id) else {
            return Vec::new();
        };
        sketch
            .constraints
            .iter()
            .enumerate()
            .filter_map(|(index, constraint)| {
                if !constraint.is_reference() {
                    return None;
                }
                let target = match *constraint {
                    Constraint::Length { line, .. } => SketchDimensionTarget::Length(line),
                    Constraint::Radius { circle, .. } => SketchDimensionTarget::Radius(circle),
                    Constraint::Diameter { circle, .. } => SketchDimensionTarget::Diameter(circle),
                    Constraint::Distance { a, b, .. } => SketchDimensionTarget::Distance { a, b },
                    Constraint::HDistance { a, b, value, .. } => SketchDimensionTarget::HDistance {
                        a,
                        b,
                        sign: value.signum(),
                    },
                    Constraint::VDistance { a, b, value, .. } => SketchDimensionTarget::VDistance {
                        a,
                        b,
                        sign: value.signum(),
                    },
                    Constraint::Angle { a, b, .. } => SketchDimensionTarget::Angle { a, b },
                    _ => return None,
                };
                let (value, anchor, name) = Self::dimension_measurement(sketch, target)?;
                Some(DimensionReadout {
                    id,
                    target,
                    label: format!("参考 {name} {}", self.dimension_display(target, value)),
                    position: self.camera.project(sketch.plane.to_world(anchor).as_vec3())
                        + Vec2::new(10.0, -18.0 + index as f32 * 3.0),
                    value,
                    reference: true,
                    expression: constraint.expression().map(str::to_owned),
                })
            })
            .collect()
    }

    fn dimension_display(&self, target: SketchDimensionTarget, value: f64) -> String {
        if matches!(target, SketchDimensionTarget::Angle { .. }) {
            format!("{value:.2}°")
        } else {
            format!(
                "{:.3} {}",
                self.units.display_value(value),
                self.units.symbol()
            )
        }
    }

    fn begin_dimension_input(
        &mut self,
        readout: DimensionReadout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let DimensionReadout {
            id,
            target,
            value,
            reference,
            position,
            expression,
            ..
        } = readout;
        let theme = self.theme.clone();
        let variables = self.numeric_variables(cx);
        let is_angle = matches!(target, SketchDimensionTarget::Angle { .. });
        let units = self.units;
        let seed = expression.unwrap_or_else(|| {
            let value = if is_angle {
                value
            } else {
                units.display_value(value)
            };
            format!("{value:.3}")
        });
        let input = cx.new(|cx| {
            let input = NumericInput::new_with_variables(seed, "", theme, variables, cx);
            if is_angle {
                input
            } else {
                input.with_units(units)
            }
        });
        let subscription = cx.subscribe_in(&input, window, |viewport, _, event, window, cx| {
            viewport.handle_numeric_input(event.clone(), window, cx);
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.dimension_target = Some((id, target));
        self.dimension_reference = reference;
        self.numeric_input = Some((input, position));
        self.numeric_input_subscription = Some(subscription);
        self.changed(window, cx);
    }

    fn handle_numeric_input(
        &mut self,
        event: NumericInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            NumericInputEvent::Commit { value, expression } => {
                if !self.commit_numeric_value(value, expression, cx) {
                    return;
                }
            }
            NumericInputEvent::Cancel => {
                if let Some(drive) = self.joint_drive.take() {
                    self.document.update(cx, |document, cx| {
                        if let Some(joint) = document
                            .joints
                            .iter_mut()
                            .find(|joint| joint.id == drive.id)
                        {
                            joint.value = drive.start_value;
                            joint.value2 = drive.start_value2;
                        }
                        document.solve_assembly();
                        cx.notify();
                    });
                    self.gizmo_readout = None;
                }
            }
        }
        self.dimension_target = None;
        self.numeric_input = None;
        self.numeric_input_subscription = None;
        window.focus(&self.focus_handle, cx);
        self.changed(window, cx);
    }

    fn commit_numeric_value(
        &mut self,
        value: f64,
        expression: String,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(drive) = self.joint_drive.take() {
            let exact = if drive.kind == JointKind::Revolute {
                value.to_radians()
            } else {
                value
            };
            self.document.update(cx, |document, cx| {
                if let Some(joint) = document
                    .joints
                    .iter_mut()
                    .find(|joint| joint.id == drive.id)
                {
                    joint.value = drive.start_value;
                    joint.value2 = drive.start_value2;
                }
                document.set_joint_value(drive.id, exact, drive.start_value2);
                cx.notify();
            });
            self.gizmo_readout = None;
            return true;
        }
        if let Some((id, target)) = self.dimension_target {
            let reference = self.dimension_reference;
            let expression = expr::contains_identifier(&expression).then_some(expression);
            let replacement = match target {
                SketchDimensionTarget::Length(line) => Constraint::Length {
                    line,
                    value: value.abs(),
                    expr: expression.clone(),
                    error: None,
                    reference,
                },
                SketchDimensionTarget::Radius(circle) => Constraint::Radius {
                    circle,
                    value: value.abs(),
                    expr: expression.clone(),
                    error: None,
                    reference,
                },
                SketchDimensionTarget::Diameter(circle) => Constraint::Diameter {
                    circle,
                    value: value.abs(),
                    expr: expression.clone(),
                    error: None,
                    reference,
                },
                SketchDimensionTarget::Distance { a, b } => Constraint::Distance {
                    a,
                    b,
                    value: value.abs(),
                    expr: expression.clone(),
                    error: None,
                    reference,
                },
                SketchDimensionTarget::HDistance { a, b, sign } => Constraint::HDistance {
                    a,
                    b,
                    value: value.abs() * sign,
                    expr: expression.clone(),
                    error: None,
                    reference,
                },
                SketchDimensionTarget::VDistance { a, b, sign } => Constraint::VDistance {
                    a,
                    b,
                    value: value.abs() * sign,
                    expr: expression.clone(),
                    error: None,
                    reference,
                },
                SketchDimensionTarget::Angle { a, b } => Constraint::Angle {
                    a,
                    b,
                    degrees: value,
                    expr: expression,
                    error: None,
                    reference,
                },
            };
            let mut committed = false;
            self.document.update(cx, |document, cx| {
                committed = document.set_dimension_constraint(id, replacement);
                if committed {
                    cx.notify();
                }
            });
            if !committed {
                eprintln!("dimension solve did not converge; dimension rejected");
            } else {
                self.sync_sketch_gpu(cx);
            }
            return committed;
        }
        if let Some(interaction) = &mut self.thread_interaction {
            let value = value.abs().max(0.001);
            if interaction.phase == ThreadPhase::Pitch {
                interaction.pitch = value;
                interaction.phase = ThreadPhase::Depth;
                self.gizmo_readout = Some((
                    "输入螺纹深度".into(),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
                self.sync_sketch_gpu(cx);
                return true;
            }
            interaction.depth = value;
            let interaction = self.thread_interaction.take().expect("checked");
            let committed = self.document.update(cx, |document, cx| {
                let committed = document.apply_thread(
                    interaction.body,
                    interaction.face,
                    interaction.external,
                    interaction.mode,
                    interaction.pitch,
                    interaction.depth,
                );
                if committed {
                    cx.notify();
                }
                committed
            });
            self.gizmo_readout = None;
            return committed;
        }
        if let Some(interaction) = &mut self.helix_interaction {
            let value = value.abs().max(0.001);
            let expression = expr::contains_identifier(&expression).then_some(expression);
            let next = match interaction.phase {
                HelixPhase::Radius => {
                    interaction.radius = value;
                    interaction.expressions[0] = expression;
                    interaction.phase = HelixPhase::Pitch;
                    "输入螺距"
                }
                HelixPhase::Pitch => {
                    interaction.pitch = value;
                    interaction.expressions[1] = expression;
                    interaction.phase = HelixPhase::Turns;
                    "输入圈数"
                }
                HelixPhase::Turns => {
                    interaction.turns = value;
                    interaction.expressions[2] = expression;
                    interaction.phase = HelixPhase::ProfileRadius;
                    "输入线径半径"
                }
                HelixPhase::ProfileRadius => {
                    interaction.profile_radius = value;
                    interaction.expressions[3] = expression;
                    let interaction = self.helix_interaction.take().expect("checked");
                    self.document.update(cx, |document, cx| {
                        if document
                            .apply_helix(
                                interaction.axis.origin,
                                interaction.axis.direction,
                                interaction.radius,
                                interaction.pitch,
                                interaction.turns,
                                interaction.profile_radius,
                                interaction.left_handed,
                            )
                            .is_some()
                        {
                            for (slot, expression) in interaction.expressions.iter().enumerate() {
                                if let Some(expression) = expression {
                                    document
                                        .set_last_history_num_expression(slot, expression.clone());
                                }
                            }
                            cx.notify();
                        }
                    });
                    self.gizmo_readout = None;
                    return true;
                }
            };
            self.gizmo_readout =
                Some((next.to_owned(), self.last_pointer + Vec2::new(14.0, -22.0)));
            return true;
        }
        if self.hole_interaction.is_some() {
            let interaction = self.hole_interaction.as_mut().expect("checked");
            let value = value.abs().max(0.1);
            match interaction.phase {
                HolePhase::Diameter => {
                    interaction.diameter = value;
                    interaction.diameter_expression =
                        expr::contains_identifier(&expression).then_some(expression);
                }
                HolePhase::Depth => {
                    interaction.kind = HoleKind::Blind {
                        depth: crate::history::Num::from_input(value, expression),
                    }
                }
                HolePhase::CounterboreDiameter => {
                    let depth = match &interaction.cut {
                        HoleCut::Counterbore { depth, .. } => depth.clone(),
                        _ => 3.0.into(),
                    };
                    interaction.cut = HoleCut::Counterbore {
                        diameter: crate::history::Num::from_input(
                            value.max(interaction.diameter + 0.1),
                            expression,
                        ),
                        depth,
                    };
                }
                HolePhase::CounterboreDepth => {
                    let diameter = match &interaction.cut {
                        HoleCut::Counterbore { diameter, .. } => diameter.clone(),
                        _ => (interaction.diameter + 2.0).into(),
                    };
                    interaction.cut = HoleCut::Counterbore {
                        diameter,
                        depth: crate::history::Num::from_input(value, expression),
                    };
                }
                HolePhase::CountersinkDiameter => {
                    interaction.cut = HoleCut::Countersink {
                        diameter: crate::history::Num::from_input(
                            value.max(interaction.diameter + 0.1),
                            expression,
                        ),
                        angle_degrees: 90.0.into(),
                    }
                }
                HolePhase::Location => return false,
            }
            self.finish_hole_drag(true, cx);
            return true;
        }
        if self.draft_interaction.is_some() {
            let interaction = self.draft_interaction.as_mut().expect("checked");
            interaction.angle_degrees = value.clamp(-45.0, 45.0);
            interaction.expression = expr::contains_identifier(&expression).then_some(expression);
            self.finish_feature_drag(true, cx);
            return true;
        }
        if let Some(interaction) = &mut self.revolve_interaction {
            interaction.angle_degrees = value.clamp(0.0, 360.0);
            interaction.expression = expr::contains_identifier(&expression).then_some(expression);
            self.finish_feature_drag(true, cx);
            return true;
        }
        if let Some(interaction) = &mut self.pattern_interaction {
            if interaction.mode != PatternMode::Linear {
                return false;
            }
            interaction.spacing = value;
            interaction.expression = expr::contains_identifier(&expression).then_some(expression);
            self.finish_feature_drag(true, cx);
            return true;
        }
        if let Some(drag) = &mut self.gizmo_drag {
            drag.current = Some(match drag.anchor {
                DragAnchor::Axis(_) => {
                    let axis = drag.handle.axis().expect("axis drag has an axis");
                    TransformOp::Translate(axis.as_dvec3() * value)
                }
                DragAnchor::Ring(_) => {
                    let axis = drag.handle.axis().expect("ring drag has an axis");
                    TransformOp::Rotate {
                        origin: drag.pivot.as_dvec3(),
                        axis: axis.as_dvec3(),
                        angle_rad: value.to_radians(),
                    }
                }
                DragAnchor::Center(_) => return false,
            });
            self.finish_gizmo_drag(true, cx);
            return true;
        }
        if let Some(interaction) = &mut self.extrude_drag {
            let direction = if interaction.drag.distance.is_sign_negative() {
                -1.0
            } else {
                1.0
            };
            if interaction.opposite_phase {
                interaction.drag.opposite_distance = value.abs();
                interaction.expressions[1] =
                    expr::contains_identifier(&expression).then_some(expression);
            } else {
                interaction.drag.distance = direction * value;
                interaction.expressions[0] =
                    expr::contains_identifier(&expression).then_some(expression);
            }
            self.finish_extrude_drag(true, cx);
            return true;
        }
        if let Some(interaction) = &mut self.profile_extrude_drag {
            let direction = if interaction.drag.distance.is_sign_negative() {
                -1.0
            } else {
                1.0
            };
            if interaction.opposite_phase {
                interaction.drag.opposite_distance = value.abs();
                interaction.expressions[1] =
                    expr::contains_identifier(&expression).then_some(expression);
            } else {
                interaction.drag.distance = direction * value;
                interaction.expressions[0] =
                    expr::contains_identifier(&expression).then_some(expression);
            }
            self.finish_extrude_drag(true, cx);
            return true;
        }
        if let Some(interaction) = &mut self.open_chain_extrude_drag {
            let direction = if interaction.drag.distance.is_sign_negative() {
                -1.0
            } else {
                1.0
            };
            if interaction.opposite_phase {
                interaction.drag.opposite_distance = value.abs();
                interaction.expressions[1] =
                    expr::contains_identifier(&expression).then_some(expression);
            } else {
                interaction.drag.distance = direction * value;
                interaction.expressions[0] =
                    expr::contains_identifier(&expression).then_some(expression);
            }
            self.finish_extrude_drag(true, cx);
            return true;
        }
        if value <= 0.0 {
            if !self
                .sketch_interaction
                .as_ref()
                .is_some_and(|interaction| interaction.edit_profile.is_some())
            {
                return false;
            }
        }
        if let Some(interaction) = &self.sketch_interaction
            && (interaction.fillet_pair.is_some() || interaction.edit_profile.is_some())
        {
            let id = interaction.id;
            let pair = interaction.fillet_pair;
            let profile = interaction.edit_profile;
            let signed = if profile.is_some() && interaction.edit_distance.is_sign_negative() {
                -value.abs()
            } else {
                value.abs()
            };
            let mut committed = false;
            self.document.update(cx, |document, cx| {
                committed = if let Some((first, second)) = pair {
                    document.fillet_sketch_lines(id, first, second, value.abs())
                } else {
                    document.offset_sketch_profile(id, profile.expect("profile"), signed)
                };
                if committed {
                    cx.notify();
                }
            });
            if committed {
                if let Some(interaction) = &mut self.sketch_interaction {
                    interaction.anchor = None;
                    interaction.cursor = None;
                    interaction.fillet_first = None;
                    interaction.fillet_pair = None;
                    interaction.edit_profile = None;
                }
                self.sync_sketch_gpu(cx);
            }
            return committed;
        }
        if let Some(interaction) = &mut self.sketch_interaction
            && matches!(interaction.tool, ToolId::Slot | ToolId::RoundedRectangle)
            && let (Some(a), Some(b)) = (interaction.anchor.take(), interaction.arc_end.take())
        {
            let generated = if interaction.tool == ToolId::Slot {
                slot(a, b, value.abs())
            } else {
                rounded_rectangle(a, b, value.abs())
            };
            let Some((entities, constraints)) = generated else {
                return false;
            };
            let id = interaction.id;
            interaction.cursor = None;
            self.document.update(cx, |document, cx| {
                document.add_sketch_primitives(id, entities, constraints);
                cx.notify();
            });
            self.gizmo_readout = None;
            self.sync_sketch_gpu(cx);
            return true;
        }
        if let Some(interaction) = &mut self.sketch_interaction
            && interaction.tool == ToolId::Polygon
            && interaction.anchor.is_some()
        {
            interaction.polygon_sides = value.round().clamp(3.0, 24.0) as usize;
            self.gizmo_readout = Some((
                format!("[−] {} 边 [+]", interaction.polygon_sides),
                self.last_pointer + Vec2::new(14.0, -22.0),
            ));
            self.sync_sketch_gpu(cx);
            return true;
        }
        if let Some(interaction) = &mut self.sketch_interaction
            && interaction.tool == ToolId::Circle
            && let Some(center) = interaction.anchor.take()
        {
            let id = interaction.id;
            interaction.cursor = None;
            self.document.update(cx, |document, cx| {
                document.add_sketch_entities(
                    id,
                    [SketchEntity::Circle {
                        center,
                        radius: value,
                    }],
                );
                cx.notify();
            });
            self.gizmo_readout = None;
            self.sync_sketch_gpu(cx);
            return true;
        }
        if let Some(interaction) = &mut self.dressup_drag {
            if self.variable_fillet && interaction.variable_start_entered {
                interaction.drag.end_radius = Some(value);
                self.finish_dressup_drag(true, cx);
            } else if self.variable_fillet {
                interaction.drag.radius = value;
                interaction.drag.end_radius = Some(value);
                interaction.variable_start_entered = true;
                self.gizmo_readout = Some((
                    "输入终点半径".into(),
                    self.last_pointer + Vec2::new(14.0, -22.0),
                ));
            } else {
                interaction.drag.radius = value;
                interaction.expression =
                    expr::contains_identifier(&expression).then_some(expression);
                self.finish_dressup_drag(true, cx);
            }
            return true;
        }
        if let Some(interaction) = &mut self.shell_drag {
            interaction.drag.thickness = value;
            interaction.expression = expr::contains_identifier(&expression).then_some(expression);
            self.finish_shell_drag(true, cx);
            return true;
        }
        if let Some(interaction) = &mut self.thicken_drag {
            interaction.thickness = value;
            interaction.expression = expr::contains_identifier(&expression).then_some(expression);
            self.finish_thicken_drag(true, cx);
            return true;
        }
        false
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key.eq_ignore_ascii_case("escape") && self.pick_popup.take().is_some() {
            self.hovered = None;
            self.changed(window, cx);
            cx.stop_propagation();
            return;
        }
        // Alt+S toggles select-through: Shift+Cmd+S stays reserved for Save As.
        if event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.platform
            && key.eq_ignore_ascii_case("s")
        {
            self.select_through = !self.select_through;
            self.pick_popup = None;
            self.hovered = None;
            self.changed(window, cx);
            cx.stop_propagation();
            return;
        }
        if key.eq_ignore_ascii_case("space")
            && !event.keystroke.modifiers.modified()
            && self.numeric_input.is_none()
        {
            self.zoom_to_face_at(self.last_pointer, window, cx);
            cx.stop_propagation();
            return;
        }
        if let Some(kind) = self.pending_reference_kind() {
            if key.eq_ignore_ascii_case("escape") {
                self.cancel_feature_interaction();
                self.changed(window, cx);
                cx.stop_propagation();
                return;
            }
            if key.eq_ignore_ascii_case("enter") {
                self.accept_default_reference(cx);
                self.changed(window, cx);
                cx.stop_propagation();
                return;
            }
            if let Some(reference) = world_reference(key, kind) {
                self.resolve_reference(reference, cx);
                self.changed(window, cx);
                cx.stop_propagation();
                return;
            }
        }
        if let Some(interaction) = &mut self.helix_interaction
            && (key.eq_ignore_ascii_case("l") || key.eq_ignore_ascii_case("r"))
        {
            interaction.left_handed = key.eq_ignore_ascii_case("l");
            self.changed(window, cx);
            cx.stop_propagation();
            return;
        }
        if key.eq_ignore_ascii_case("t")
            && !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && let Some(interaction) = &mut self.sketch_interaction
            && matches!(interaction.tool, ToolId::Line | ToolId::TangentArc)
        {
            interaction.tangent_arc = !interaction.tangent_arc;
            self.gizmo_readout = Some((
                if interaction.tangent_arc {
                    "切线弧 · T 切换直线".to_owned()
                } else {
                    "直线 · T 切换切线弧".to_owned()
                },
                self.last_pointer + Vec2::new(14.0, -22.0),
            ));
            self.sync_sketch_gpu(cx);
            self.changed(window, cx);
            cx.stop_propagation();
            return;
        }
        if let NumericDragTransition::Freeze(seed) = Self::numeric_drag_transition(
            self.supports_numeric_input(),
            self.numeric_input.is_some(),
            event,
        ) {
            self.begin_numeric_input(seed, window, cx);
            cx.stop_propagation();
        } else if event.keystroke.modifiers.platform && key.eq_ignore_ascii_case("z") {
            let redo = event.keystroke.modifiers.shift;
            self.document.update(cx, |document, cx| {
                if redo {
                    document.redo();
                } else {
                    document.undo();
                }
                cx.notify();
            });
            self.changed(window, cx);
        } else if key.eq_ignore_ascii_case("tab") {
            self.document.update(cx, |document, cx| {
                document.selection.filter = document.selection.filter.next();
                cx.notify();
            });
            self.changed(window, cx);
        } else if key.eq_ignore_ascii_case("escape") {
            if let Some(interaction) = self.m6_interaction.take() {
                if matches!(interaction, M6Interaction::Split { .. }) {
                    self.section_enabled = false;
                }
                self.gizmo_readout = None;
                self.changed(window, cx);
                return;
            }
            if self.exit_modes() {
                self.sync_scene(cx);
                cx.emit(ViewportEvent::ModesExited);
                self.changed(window, cx);
                return;
            }
            if self.finish_marquee(false, cx) {
                self.changed(window, cx);
                return;
            }
            if let Some(drag) = self.sketch_entity_drag.take() {
                self.document.update(cx, |document, cx| {
                    document.finish_sketch_drag(drag.id, drag.start, false);
                    cx.notify();
                });
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
            if self.revolve_interaction.is_some()
                || self.hole_interaction.is_some()
                || self.draft_interaction.is_some()
                || self.pattern_interaction.is_some()
                || self.sketch_pattern_interaction.is_some()
                || self.thread_interaction.is_some()
                || self.pending_reference.is_some()
            {
                self.cancel_feature_interaction();
                self.sync_sketch_gpu(cx);
                self.changed(window, cx);
                return;
            }
            if let Some(interaction) = &mut self.sketch_interaction {
                if interaction.anchor.is_some() {
                    interaction.anchor = None;
                    interaction.anchor_ref = None;
                    interaction.arc_end = None;
                    interaction.arc_end_ref = None;
                    interaction.chain_start = None;
                    interaction.cursor = None;
                    interaction.spline_points.clear();
                    interaction.spline_start_ref = None;
                    interaction.spline_end_ref = None;
                    self.gizmo_readout = None;
                    self.sync_sketch_gpu(cx);
                } else if !self.document.read(cx).selection.items.is_empty() {
                    self.document.update(cx, |document, cx| {
                        document.selection.clear();
                        cx.notify();
                    });
                    self.sync_sketch_gpu(cx);
                } else {
                    // First Escape only leaves the drawing tool: sketch mode
                    // stays active so profiles remain clickable/pullable; the
                    // next Escape (handled below) exits the sketch itself.
                    self.sketch_interaction = None;
                    self.gizmo_readout = None;
                    self.sync_sketch_gpu(cx);
                }
                self.changed(window, cx);
                return;
            }
            if self.document.read(cx).active_sketch.is_some() {
                if self.document.read(cx).selection.items.is_empty() {
                    self.exit_sketch(cx);
                } else {
                    self.document.update(cx, |document, cx| {
                        document.selection.clear();
                        cx.notify();
                    });
                    self.sync_sketch_gpu(cx);
                }
                self.changed(window, cx);
                return;
            }
            if self.dressup_drag.is_some() {
                self.finish_dressup_drag(false, cx);
                self.changed(window, cx);
                return;
            }
            if self.shell_drag.is_some() {
                self.finish_shell_drag(false, cx);
                self.changed(window, cx);
                return;
            }
            if self.thicken_drag.is_some() {
                self.finish_thicken_drag(false, cx);
                self.changed(window, cx);
                return;
            }
            if self.active_drag_tool.is_some() {
                self.active_drag_tool = None;
                self.gizmo_readout = None;
                self.changed(window, cx);
                return;
            }
            if self.extrude_drag.is_some()
                || self.profile_extrude_drag.is_some()
                || self.open_chain_extrude_drag.is_some()
            {
                self.finish_extrude_drag(false, cx);
                self.changed(window, cx);
                return;
            }
            if self.gizmo_drag.is_some() {
                self.finish_gizmo_drag(false, cx);
                self.changed(window, cx);
                return;
            }
            self.document.update(cx, |document, cx| {
                document.selection.clear();
                cx.notify();
            });
            self.changed(window, cx);
        } else if key.eq_ignore_ascii_case("enter") && self.m6_interaction.is_some() {
            self.commit_m6(cx);
            self.changed(window, cx);
        } else if key.eq_ignore_ascii_case("enter") && self.commit_pending_spline(cx) {
            self.sync_sketch_gpu(cx);
            self.changed(window, cx);
        } else if let Some(M6Interaction::Align { axes, .. }) = &mut self.m6_interaction
            && matches!(key.to_ascii_lowercase().as_str(), "x" | "y" | "z")
        {
            let index = match key.to_ascii_lowercase().as_str() {
                "x" => 0,
                "y" => 1,
                _ => 2,
            };
            axes[index] = !axes[index];
            self.gizmo_readout = Some((
                format!(
                    "[X{}] [Y{}] [Z{}] · Enter",
                    if axes[0] { "✓" } else { "" },
                    if axes[1] { "✓" } else { "" },
                    if axes[2] { "✓" } else { "" }
                ),
                self.last_pointer + Vec2::new(14.0, -22.0),
            ));
            self.changed(window, cx);
        } else if key.eq_ignore_ascii_case("enter")
            && let Some(interaction) = &mut self.sketch_interaction
            && matches!(interaction.tool, ToolId::Line | ToolId::TangentArc)
            && interaction.anchor.is_some()
        {
            interaction.anchor = None;
            interaction.anchor_ref = None;
            interaction.chain_start = None;
            interaction.cursor = None;
            self.sync_sketch_gpu(cx);
            self.changed(window, cx);
        } else if event.keystroke.modifiers.platform
            && let Some(view) = StandardView::from_digit(key)
        {
            self.go_to_standard_view(view, window, cx);
        }
    }

    fn render_image(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        width: u32,
        height: u32,
    ) {
        let selection = self.document.read(cx).selection.items.clone();
        let gizmo = self.gizmo_state(cx);
        let extrude_arrow = self.tool_arrow_state(cx);
        let rendered = self.renderer.render(
            &self.camera,
            width,
            height,
            self.show_grid,
            self.display_mode,
            self.analysis,
            self.visualize,
            self.hovered,
            &selection,
            gizmo,
            extrude_arrow,
            self.section_arrow_state(),
            self.section_plane(),
            OrientationCubeRender {
                device_scale: self.device_scale,
                hovered: self.hovered_cube,
            },
        );
        Self::dump_frame(&rendered);
        let pixels = RgbaImage::from_raw(rendered.width, rendered.height, rendered.bgra)
            .expect("renderer returned an invalid BGRA image size");
        let latest_frame = Arc::new(RenderImage::new(smallvec![Frame::new(pixels)]));
        if let Some(current) = self.current_rendered_frame.take() {
            if let Some(frame) = self.previous_rendered_frame.take()
                && frame.id != current.id
            {
                let _ = window.drop_image(frame);
            }
            self.previous_rendered_frame = Some(current);
        }
        self.current_rendered_frame = Some(latest_frame);
    }

    fn dump_frame(rendered: &renderer::RenderedFrame) {
        save_dump_frame(rendered);
    }

    /// Returns the most recently rendered frame as `(width, height, BGRA bytes)`.
    ///
    /// Reads the pixels back from the live [`RenderImage`], so no per-frame copy
    /// is kept; used by the screenshot command to encode a PNG.
    pub fn latest_frame(&self) -> Option<(u32, u32, Vec<u8>)> {
        let frame = self.current_rendered_frame.as_ref()?;
        let size = frame.size(0);
        let bytes = frame.as_bytes(0)?.to_vec();
        Some((size.width.0 as u32, size.height.0 as u32, bytes))
    }
}

impl Render for Viewport {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        let width = (f32::from(viewport.width) * scale).round().max(1.0) as u32;
        let height = (f32::from(viewport.height) * scale).round().max(1.0) as u32;
        if self.rendered_size != (width, height) {
            self.rendered_size = (width, height);
            self.camera.viewport_size = Vec2::new(width as f32, height as f32);
            self.dirty = true;
        }
        self.device_scale = scale;
        self.sync_scene(cx);

        let now = Instant::now();
        let dt = (now - self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        let animating = self.camera.tick(dt);
        if self.dirty || animating {
            self.render_image(window, cx, width, height);
            self.dirty = false;
        }
        if animating
            || self.dragging.is_some()
            || self.gizmo_drag.is_some()
            || self.extrude_drag.is_some()
            || self.profile_extrude_drag.is_some()
            || self.open_chain_extrude_drag.is_some()
            || self.dressup_drag.is_some()
            || self.shell_drag.is_some()
            || self.thicken_drag.is_some()
            || self.revolve_interaction.is_some()
            || self.hole_interaction.is_some()
            || self.draft_interaction.is_some()
            || self.pattern_interaction.is_some()
            || self.sketch_pattern_interaction.is_some()
            || self.thread_interaction.is_some()
            || self.pending_reference.is_some()
            || self.cube_interaction.is_some()
            || self.section_drag.is_some()
        {
            window.request_animation_frame();
        }

        self.extrude_badges = self
            .revolve_interaction
            .as_ref()
            .map(|interaction| {
                (
                    ExtrudeMode::ALL
                        .into_iter()
                        .map(|mode| (mode, mode == interaction.mode))
                        .collect(),
                    self.last_pointer + Vec2::new(14.0, 16.0),
                )
            })
            .or_else(|| {
                (self.active_drag_tool.is_none()
                    && self.revolve_interaction.is_none()
                    && self.pattern_interaction.is_none()
                    && self.sketch_pattern_interaction.is_none())
                .then(|| self.extrude_arrow_state(cx))
                .flatten()
                .map(|arrow| {
                    (
                        ExtrudeMode::ALL
                            .into_iter()
                            .map(|mode| (mode, mode == self.extrude_mode))
                            .collect(),
                        self.camera.project(arrow.origin) + Vec2::new(18.0, 18.0),
                    )
                })
            });
        self.extrude_side_badges = (self.revolve_interaction.is_none())
            .then(|| {
                self.extrude_badges.as_ref().map(|(_, position)| {
                    (
                        ExtrudeSideMode::ALL
                            .into_iter()
                            .map(|mode| (mode, mode == self.extrude_side_mode))
                            .collect(),
                        *position + Vec2::new(0.0, 28.0 * scale),
                    )
                })
            })
            .flatten();

        let image = self.current_rendered_frame.clone();
        let dressup_badges = (self.active_drag_tool == Some(ToolId::Fillet)
            || self
                .dressup_drag
                .as_ref()
                .is_some_and(|interaction| interaction.drag.fillet))
        .then_some((
            self.variable_fillet,
            self.last_pointer + Vec2::new(14.0, 16.0),
        ));
        let thread_badges = self.thread_interaction.as_ref().map(|interaction| {
            (
                interaction.external,
                interaction.mode,
                self.last_pointer + Vec2::new(14.0, 16.0),
            )
        });
        let readout = self
            .numeric_input
            .is_none()
            .then(|| self.gizmo_readout.clone())
            .flatten();
        let numeric_input = self.numeric_input.clone();
        let dimension_edit = self.dimension_target.is_some();
        let dimension_reference = self.dimension_reference;
        let dimensions = if self.numeric_input.is_none() {
            let selected = self.dimension_readouts(cx);
            let mut references = self.reference_dimension_readouts(cx);
            references.retain(|reference| {
                !selected
                    .iter()
                    .any(|readout| readout.target == reference.target)
            });
            references.extend(selected);
            references
        } else {
            Vec::new()
        };
        let badges = self.extrude_badges.clone();
        let side_badges = self.extrude_side_badges.clone();
        let transform_badges = self
            .gizmo_drag
            .as_ref()
            .map(|_| (self.gizmo_repeat, self.last_pointer + Vec2::new(14.0, 16.0)));
        let helix_badges = self.helix_interaction.as_ref().map(|interaction| {
            (
                interaction.left_handed,
                self.last_pointer + Vec2::new(14.0, 16.0),
            )
        });
        let hole_badges = self.hole_interaction.as_ref().and_then(|interaction| {
            interaction.at.map(|_| {
                (
                    matches!(interaction.kind, HoleKind::Blind { .. }),
                    match interaction.cut {
                        HoleCut::Counterbore { .. } => 0usize,
                        HoleCut::Countersink { .. } => 1,
                        HoleCut::None => 2,
                    },
                    self.last_pointer + Vec2::new(14.0, 16.0),
                )
            })
        });
        let pattern_badges = self
            .pattern_interaction
            .as_ref()
            .map(|interaction| {
                (
                    interaction.mode,
                    interaction.count,
                    self.last_pointer + Vec2::new(14.0, 16.0),
                )
            })
            .or_else(|| {
                self.sketch_pattern_interaction.as_ref().map(|interaction| {
                    (
                        interaction.mode,
                        interaction.count,
                        self.last_pointer + Vec2::new(14.0, 16.0),
                    )
                })
            });
        let polygon_badges = self.sketch_interaction.as_ref().and_then(|interaction| {
            (interaction.tool == ToolId::Polygon && interaction.anchor.is_some()).then_some((
                interaction.polygon_sides,
                self.last_pointer + Vec2::new(14.0, 4.0),
            ))
        });
        let m6_badges = match &self.m6_interaction {
            Some(M6Interaction::Align { axes, .. }) => Some((Some(*axes), self.last_pointer)),
            Some(M6Interaction::Split { .. }) => Some((None, self.last_pointer)),
            _ => None,
        };
        let measurement = (self.measure_anchors.len() == 2).then(|| {
            let measurement = Measurement {
                first: self.measure_anchors[0],
                second: self.measure_anchors[1],
            };
            let position = self
                .camera
                .project((measurement.first + measurement.second) * 0.5)
                + Vec2::new(14.0, -18.0);
            (measurement, position)
        });
        let marquee = self
            .marquee
            .as_ref()
            .filter(|marquee| marquee.active)
            .map(|marquee| {
                let rect = ScreenRect::from_points(marquee.start, marquee.current);
                (
                    rect,
                    marquee_mode(marquee.start, marquee.current),
                    self.theme.accent,
                )
            });
        let active_sketch = self.document.read(cx).active_sketch.is_some();
        let active_filter = self.document.read(cx).selection.filter;
        let filter_hud = (!active_sketch).then_some(active_filter);
        let select_through = self.select_through;
        let pick_popup = self.pick_popup.clone().map(|popup| {
            let document = self.document.read(cx);
            let rows = popup
                .candidates
                .iter()
                .map(|candidate| {
                    let body_id = candidate.item.body_id();
                    let body = body_id.and_then(|id| {
                        document
                            .bodies
                            .iter()
                            .enumerate()
                            .find(|(_, body)| body.id == id)
                    });
                    let body_name = body.map_or("未命名", |(_, body)| body.name.as_str());
                    let body_index = body.map_or(0, |(index, _)| index + 1);
                    let label = match candidate.item {
                        SelItem::Body(_) => format!("体 · {body_name} · {body_index}"),
                        SelItem::Face(_, face) => {
                            format!("面 · {body_name} · {}", face + 1)
                        }
                        SelItem::Edge(_, edge) => {
                            format!("边 · {body_name} · {}", edge + 1)
                        }
                        _ => String::new(),
                    };
                    (*candidate, label)
                })
                .collect::<Vec<_>>();
            (popup.position, popup.shift, rows)
        });
        div()
            .id("viewport")
            .relative()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(Self::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(Self::mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Right, cx.listener(Self::mouse_up))
            .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::mouse_up))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .on_pinch(cx.listener(Self::pinch))
            .on_key_down(cx.listener(Self::key_down))
            .when_some(image, |element, image| {
                element.child(img(ImageSource::Render(image)).size_full())
            })
            .when_some(marquee, |element, (rect, mode, accent)| {
                let mut fill = accent;
                fill.a = if mode == MarqueeMode::Window {
                    0.10
                } else {
                    0.16
                };
                element.child(
                    div()
                        .absolute()
                        .left(px(rect.minimum.x / scale))
                        .top(px(rect.minimum.y / scale))
                        .w(px((rect.maximum.x - rect.minimum.x) / scale))
                        .h(px((rect.maximum.y - rect.minimum.y) / scale))
                        .border_1()
                        .border_color(accent)
                        .when(mode == MarqueeMode::Crossing, |rectangle| {
                            rectangle.border_dashed()
                        })
                        .bg(fill),
                )
            })
            .when_some(filter_hud, |element, active_filter| {
                let theme = &self.theme;
                element.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom(theme.space(3.0))
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(2.0))
                                .p(theme.space(1.0))
                                .rounded(px(theme.radius_panel))
                                .border_1()
                                .border_color(theme.border)
                                .bg(theme.panel)
                                .shadow(theme.shadow.clone())
                                .when(select_through, |row| {
                                    row.child(
                                        div()
                                            .px(theme.space(2.0))
                                            .py(theme.space(1.0))
                                            .rounded(px(theme.radius_control))
                                            .bg(theme.accent_wash)
                                            .text_color(theme.accent)
                                            .text_size(px(theme.text_sm + 1.0))
                                            .child("穿透选择"),
                                    )
                                    .child(div().w(px(1.0)).h(px(18.0)).bg(theme.border))
                                })
                                .children(
                                    [
                                        (SelectionFilter::Body, "体"),
                                        (SelectionFilter::Face, "面"),
                                        (SelectionFilter::Edge, "边"),
                                        (SelectionFilter::Auto, "自动"),
                                    ]
                                    .into_iter()
                                    .enumerate()
                                    .map(
                                        |(index, (filter, label))| {
                                            div()
                                                .id(("selection-filter", index))
                                                .px(theme.space(2.0))
                                                .py(theme.space(1.0))
                                                .rounded(px(theme.radius_control))
                                                .text_size(px(theme.text_sm + 1.0))
                                                .text_color(if filter == active_filter {
                                                    theme.accent
                                                } else {
                                                    theme.text_muted
                                                })
                                                .when(filter == active_filter, |chip| {
                                                    chip.bg(theme.accent_wash)
                                                })
                                                .hover(|style| style.bg(theme.hover))
                                                .active(|style| style.bg(theme.active))
                                                .cursor_pointer()
                                                .child(label)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    cx.listener(move |this, _, window, cx| {
                                                        cx.stop_propagation();
                                                        this.document.update(cx, |document, cx| {
                                                            document.selection.filter = filter;
                                                            cx.notify();
                                                        });
                                                        this.pick_popup = None;
                                                        this.changed(window, cx);
                                                    }),
                                                )
                                        },
                                    ),
                                ),
                        ),
                )
            })
            .when_some(pick_popup, |element, (position, shift, rows)| {
                let theme = &self.theme;
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .w(px(190.0))
                        .flex()
                        .flex_col()
                        .gap(px(1.0))
                        .p(theme.space(1.0))
                        .rounded(px(theme.radius_panel))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.elevated)
                        .shadow(theme.shadow.clone())
                        .children(rows.into_iter().enumerate().map(
                            |(index, (candidate, label))| {
                                let item = candidate.item;
                                div()
                                    .id(("overlap-pick", index))
                                    .px(theme.space(2.0))
                                    .py(theme.space(1.5))
                                    .rounded(px(theme.radius_control))
                                    .text_size(px(theme.text_sm + 1.0))
                                    .text_color(theme.text)
                                    .hover(|style| style.bg(theme.hover))
                                    .active(|style| style.bg(theme.active))
                                    .cursor_pointer()
                                    .child(label)
                                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                        this.hovered = hovered.then_some(item);
                                        this.dirty = true;
                                        cx.notify();
                                    }))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            cx.stop_propagation();
                                            this.document.update(cx, |document, cx| {
                                                document.selection.apply(item, shift);
                                                cx.notify();
                                            });
                                            this.pick_popup = None;
                                            this.hovered = Some(item);
                                            this.changed(window, cx);
                                        }),
                                    )
                            },
                        )),
                )
            })
            .when_some(readout, |element, (label, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgba(0x20262eee))
                        .text_color(rgba(0xf3f5f7ff))
                        .text_size(px(12.0))
                        .child(label),
                )
            })
            .when_some(hole_badges, |element, (blind, cut, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .children(
                            [("通孔", !blind, 0usize), ("盲孔", blind, 1usize)]
                                .into_iter()
                                .map(|(label, active, index)| {
                                    div()
                                        .id(("hole-kind", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(if active {
                                            rgba(0xff6a2fff)
                                        } else {
                                            rgba(0x20262eee)
                                        })
                                        .text_color(rgba(0xf3f5f7ff))
                                        .text_size(px(11.0))
                                        .child(label)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                cx.stop_propagation();
                                                if let Some(interaction) =
                                                    &mut this.hole_interaction
                                                {
                                                    interaction.kind = if index == 0 {
                                                        HoleKind::Through
                                                    } else {
                                                        HoleKind::Blind { depth: 15.0.into() }
                                                    };
                                                }
                                                cx.notify();
                                            }),
                                        )
                                }),
                        )
                        .child(
                            div().flex().gap_1().children(
                                [("沉孔", 0usize), ("锥孔", 1usize), ("无", 2usize)]
                                    .into_iter()
                                    .map(|(label, index)| {
                                        div()
                                            .id(("hole-cut", index))
                                            .px_2()
                                            .py_1()
                                            .rounded_md()
                                            .bg(if cut == index {
                                                rgba(0xff6a2fff)
                                            } else {
                                                rgba(0x20262eee)
                                            })
                                            .text_color(rgba(0xf3f5f7ff))
                                            .text_size(px(11.0))
                                            .child(label)
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    cx.stop_propagation();
                                                    if let Some(interaction) =
                                                        &mut this.hole_interaction
                                                    {
                                                        interaction.cut = match index {
                                                            0 => HoleCut::Counterbore {
                                                                diameter: (interaction.diameter
                                                                    * 1.6)
                                                                    .into(),
                                                                depth: 3.0.into(),
                                                            },
                                                            1 => HoleCut::Countersink {
                                                                diameter: (interaction.diameter
                                                                    * 1.6)
                                                                    .into(),
                                                                angle_degrees: 90.0.into(),
                                                            },
                                                            _ => HoleCut::None,
                                                        };
                                                    }
                                                    cx.notify();
                                                }),
                                            )
                                    }),
                            ),
                        ),
                )
            })
            .when_some(polygon_badges, |element, (sides, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_1()
                        .rounded_md()
                        .bg(rgba(0x20262eee))
                        .text_color(rgba(0xf3f5f7ff))
                        .child(
                            div()
                                .id("polygon-minus")
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .child("−")
                                .on_click(cx.listener(|viewport, _, _, cx| {
                                    if let Some(interaction) = &mut viewport.sketch_interaction {
                                        interaction.polygon_sides =
                                            interaction.polygon_sides.saturating_sub(1).max(3);
                                    }
                                    viewport.sync_sketch_gpu(cx);
                                    cx.notify();
                                })),
                        )
                        .child(div().text_size(px(12.0)).child(format!("{sides} 边")))
                        .child(
                            div()
                                .id("polygon-plus")
                                .px_2()
                                .py_1()
                                .cursor_pointer()
                                .child("+")
                                .on_click(cx.listener(|viewport, _, _, cx| {
                                    if let Some(interaction) = &mut viewport.sketch_interaction {
                                        interaction.polygon_sides =
                                            (interaction.polygon_sides + 1).min(24);
                                    }
                                    viewport.sync_sketch_gpu(cx);
                                    cx.notify();
                                })),
                        ),
                )
            })
            .when_some(measurement, |element, (measurement, position)| {
                let delta = measurement.delta();
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgba(0x20262eee))
                        .border_1()
                        .border_color(rgba(0xff7a2fff))
                        .text_color(rgba(0xf3f5f7ff))
                        .child(div().text_size(px(12.0)).child(format!(
                            "Distance {:.3} {}",
                            self.units.display_value(f64::from(measurement.distance())),
                            self.units.symbol()
                        )))
                        .child(
                            div()
                                .text_size(px(10.0))
                                .text_color(rgba(0xaeb6c0ff))
                                .child(format!(
                                    "ΔX {:+.3}   ΔY {:+.3}   ΔZ {:+.3}",
                                    delta.x, delta.y, delta.z
                                )),
                        ),
                )
            })
            .when_some(numeric_input, |element, (input, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(input)
                        .when(dimension_edit, |row| {
                            row.child(
                                div()
                                    .id("dimension-reference-toggle")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .bg(if dimension_reference {
                                        rgba(0xff6a2fff)
                                    } else {
                                        rgba(0x20262eee)
                                    })
                                    .text_color(rgba(0xf3f5f7ff))
                                    .text_size(px(11.0))
                                    .child("参考")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            cx.stop_propagation();
                                            this.dimension_reference = !this.dimension_reference;
                                            cx.notify();
                                        }),
                                    ),
                            )
                        }),
                )
            })
            .children(dimensions.into_iter().enumerate().map(|(index, readout)| {
                let DimensionReadout {
                    id,
                    target,
                    label,
                    position,
                    value,
                    reference,
                    expression,
                } = readout;
                div()
                    .id(("sketch-dimension", index))
                    .absolute()
                    .left(px(position.x / scale))
                    .top(px(position.y / scale))
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(rgba(0x20262eee))
                    .border_1()
                    .border_color(rgba(0xff7a2fff))
                    .text_color(rgba(0xf3f5f7ff))
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .child(label)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.begin_dimension_input(
                                DimensionReadout {
                                    id,
                                    target,
                                    label: String::new(),
                                    position,
                                    value,
                                    reference,
                                    expression: expression.clone(),
                                },
                                window,
                                cx,
                            );
                        }),
                    )
            }))
            .when_some(badges, |element, (badges, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children(
                            badges
                                .into_iter()
                                .enumerate()
                                .map(|(index, (mode, active))| {
                                    div()
                                        .id(("extrude-mode", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(if active {
                                            rgba(0xff6a2fff)
                                        } else {
                                            rgba(0x20262eee)
                                        })
                                        .text_color(rgba(0xf3f5f7ff))
                                        .text_size(px(11.0))
                                        .child(mode.label())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                this.set_extrude_mode(mode, window, cx)
                                            }),
                                        )
                                }),
                        ),
                )
            })
            .when_some(dressup_badges, |element, (variable, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children([false, true].into_iter().map(|mode| {
                            div()
                                .id(if mode {
                                    "variable-fillet"
                                } else {
                                    "constant-fillet"
                                })
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(if variable == mode {
                                    rgba(0xff6a2fff)
                                } else {
                                    rgba(0x20262eee)
                                })
                                .text_color(rgba(0xf3f5f7ff))
                                .text_size(px(11.0))
                                .child(if mode { "可变" } else { "恒定" })
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.set_variable_fillet(mode, window, cx)
                                    }),
                                )
                        })),
                )
            })
            .when_some(thread_badges, |element, (external, mode, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children([(true, "外螺纹"), (false, "内螺纹")].into_iter().map(
                            |(value, label)| {
                                div()
                                    .id(if value {
                                        "external-thread"
                                    } else {
                                        "internal-thread"
                                    })
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(if external == value {
                                        rgba(0xff6a2fff)
                                    } else {
                                        rgba(0x20262eee)
                                    })
                                    .text_color(rgba(0xf3f5f7ff))
                                    .text_size(px(11.0))
                                    .child(label)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.set_thread_external(value, window, cx)
                                        }),
                                    )
                            },
                        ))
                        .children(
                            [
                                (crate::document::ThreadMode::Cosmetic, "装饰"),
                                (crate::document::ThreadMode::Modeled, "实体"),
                            ]
                            .into_iter()
                            .map(|(value, label)| {
                                div()
                                    .id(if value == crate::document::ThreadMode::Cosmetic {
                                        "cosmetic-thread"
                                    } else {
                                        "modeled-thread"
                                    })
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(if mode == value {
                                        rgba(0xff6a2fff)
                                    } else {
                                        rgba(0x20262eee)
                                    })
                                    .text_color(rgba(0xf3f5f7ff))
                                    .text_size(px(11.0))
                                    .child(label)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.set_thread_mode(value, window, cx)
                                        }),
                                    )
                            }),
                        ),
                )
            })
            .when_some(side_badges, |element, (badges, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children(
                            badges
                                .into_iter()
                                .enumerate()
                                .map(|(index, (mode, active))| {
                                    div()
                                        .id(("extrude-side-mode", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(if active {
                                            rgba(0xff6a2fff)
                                        } else {
                                            rgba(0x20262eee)
                                        })
                                        .text_color(rgba(0xf3f5f7ff))
                                        .text_size(px(11.0))
                                        .child(mode.label())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                this.set_extrude_side_mode(mode, window, cx)
                                            }),
                                        )
                                }),
                        ),
                )
            })
            .when_some(pattern_badges, |element, (active_mode, count, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children(
                            [PatternMode::Linear, PatternMode::Circular]
                                .into_iter()
                                .enumerate()
                                .map(|(index, mode)| {
                                    div()
                                        .id(("pattern-mode", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(if mode == active_mode {
                                            rgba(0xff6a2fff)
                                        } else {
                                            rgba(0x20262eee)
                                        })
                                        .text_color(rgba(0xf3f5f7ff))
                                        .text_size(px(11.0))
                                        .child(mode.label())
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                this.set_pattern_mode(mode, window, cx)
                                            }),
                                        )
                                }),
                        )
                        .child(
                            div()
                                .id("pattern-count-minus")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x20262eee))
                                .text_color(rgba(0xf3f5f7ff))
                                .text_size(px(11.0))
                                .child("−")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.change_pattern_count(-1, window, cx)
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x20262eee))
                                .text_color(rgba(0xf3f5f7ff))
                                .text_size(px(11.0))
                                .child(count.to_string()),
                        )
                        .child(
                            div()
                                .id("pattern-count-plus")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x20262eee))
                                .text_color(rgba(0xf3f5f7ff))
                                .text_size(px(11.0))
                                .child("+")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        this.change_pattern_count(1, window, cx)
                                    }),
                                ),
                        ),
                )
            })
            .when_some(transform_badges, |element, (active, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children([1usize, 2, 3, 5].into_iter().map(|count| {
                            div()
                                .id(("transform-repeat", count))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(if count == active {
                                    rgba(0xff6a2fff)
                                } else {
                                    rgba(0x20262eee)
                                })
                                .text_color(rgba(0xf3f5f7ff))
                                .text_size(px(11.0))
                                .child(format!("x{count}"))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.gizmo_repeat = count;
                                        this.changed(window, cx);
                                    }),
                                )
                        })),
                )
            })
            .when_some(helix_badges, |element, (left_handed, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px(position.y / scale))
                        .flex()
                        .gap_1()
                        .children([(true, "左旋"), (false, "右旋")].into_iter().map(
                            |(left, label)| {
                                div()
                                    .id(("helix-hand", usize::from(left)))
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .bg(if left == left_handed {
                                        rgba(0xff6a2fff)
                                    } else {
                                        rgba(0x20262eee)
                                    })
                                    .text_color(rgba(0xf3f5f7ff))
                                    .text_size(px(11.0))
                                    .child(label)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            if let Some(interaction) = &mut this.helix_interaction {
                                                interaction.left_handed = left;
                                            }
                                            this.changed(window, cx);
                                        }),
                                    )
                            },
                        )),
                )
            })
            .when_some(m6_badges, |element, (axes, position)| {
                element.child(
                    div()
                        .absolute()
                        .left(px(position.x / scale))
                        .top(px((position.y + 18.0) / scale))
                        .flex()
                        .gap_1()
                        .when_some(axes, |row, axes| {
                            row.children(["X", "Y", "Z"].into_iter().enumerate().map(
                                |(index, label)| {
                                    div()
                                        .id(("align-axis", index))
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(if axes[index] {
                                            rgba(0xff6a2fff)
                                        } else {
                                            rgba(0x20262eee)
                                        })
                                        .text_color(rgba(0xf3f5f7ff))
                                        .text_size(px(11.0))
                                        .child(label)
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.toggle_align_axis(index, window, cx);
                                            }),
                                        )
                                },
                            ))
                        })
                        .child(
                            div()
                                .id("m6-confirm")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(rgba(0x3b82f6ff))
                                .text_color(rgba(0xffffffff))
                                .text_size(px(11.0))
                                .child("确认")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, window, cx| {
                                        cx.stop_propagation();
                                        this.confirm_m6(window, cx);
                                    }),
                                ),
                        ),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use gpui::{KeyDownEvent, Keystroke};

    use super::{
        MarqueeMode, Measurement, NumericDragTransition, PickCandidate, ReferenceGeometry,
        ReferenceKind, SceneMesh, ScreenRect, Viewport, ambiguous_candidates, exploded_offset,
        marquee_mode, nearest_face_per_body, point_is_clipped, resolve_selection_candidate,
        scene_upload_list, screen_bounds_match, world_reference,
    };
    use crate::{
        document::{BodyId, BodyKind, Material, SelItem},
        kernel::{BodyMesh, tessellate},
        pick::{PickBody, pick_all},
    };
    use glam::{DVec3, Vec2, Vec3};

    #[test]
    fn clip_predicate_keeps_plane_and_negative_half_space() {
        assert!(!point_is_clipped(Vec3::new(2.0, 3.0, 4.0), Vec3::Y, 3.0));
        assert!(!point_is_clipped(Vec3::new(2.0, -1.0, 4.0), Vec3::Y, 3.0));
        assert!(point_is_clipped(Vec3::new(2.0, 3.01, 4.0), Vec3::Y, 3.0));
    }

    #[test]
    fn isolate_upload_list_excludes_other_and_hidden_bodies() {
        let shape = Arc::new(occt::Shape::cube(1.0).unwrap());
        let scene = [
            SceneMesh {
                id: BodyId(1),
                visible: true,
                shape: Arc::clone(&shape),
                mesh: BodyMesh::default(),
                material: Material::default(),
                kind: BodyKind::Solid,
                pose: glam::Mat4::IDENTITY,
            },
            SceneMesh {
                id: BodyId(2),
                visible: true,
                shape: Arc::clone(&shape),
                mesh: BodyMesh::default(),
                material: Material::default(),
                kind: BodyKind::Solid,
                pose: glam::Mat4::IDENTITY,
            },
            SceneMesh {
                id: BodyId(3),
                visible: false,
                shape,
                mesh: BodyMesh::default(),
                material: Material::default(),
                kind: BodyKind::Solid,
                pose: glam::Mat4::IDENTITY,
            },
        ];
        let isolated = HashSet::from([BodyId(2), BodyId(3)]);
        let ids: Vec<_> = scene_upload_list(&scene, Some(&isolated))
            .into_iter()
            .map(|(id, _, _, _)| id)
            .collect();
        assert_eq!(ids, vec![BodyId(2)]);
    }

    #[test]
    fn measurement_reports_distance_and_signed_deltas() {
        let measurement = Measurement {
            first: Vec3::new(1.0, 5.0, -2.0),
            second: Vec3::new(4.0, 1.0, 10.0),
        };
        assert_eq!(measurement.delta(), Vec3::new(3.0, -4.0, 12.0));
        assert!((measurement.distance() - 13.0).abs() < 1.0e-6);
    }

    #[test]
    fn marquee_direction_follows_horizontal_drag() {
        let start = Vec2::new(100.0, 50.0);
        assert_eq!(
            marquee_mode(start, Vec2::new(140.0, 20.0)),
            MarqueeMode::Window
        );
        assert_eq!(
            marquee_mode(start, Vec2::new(60.0, 80.0)),
            MarqueeMode::Crossing
        );
    }

    #[test]
    fn projected_screen_bounds_distinguish_window_and_crossing() {
        let selection = ScreenRect::from_points(Vec2::ZERO, Vec2::splat(100.0));
        let contained = ScreenRect::from_points(Vec2::splat(20.0), Vec2::splat(80.0));
        let overlapping = ScreenRect::from_points(Vec2::new(80.0, 20.0), Vec2::new(120.0, 80.0));
        let outside = ScreenRect::from_points(Vec2::splat(110.0), Vec2::splat(130.0));

        assert!(screen_bounds_match(
            selection,
            contained,
            MarqueeMode::Window
        ));
        assert!(!screen_bounds_match(
            selection,
            overlapping,
            MarqueeMode::Window
        ));
        assert!(screen_bounds_match(
            selection,
            overlapping,
            MarqueeMode::Crossing
        ));
        assert!(!screen_bounds_match(
            selection,
            outside,
            MarqueeMode::Crossing
        ));
    }

    #[test]
    fn overlapping_ray_hits_are_ambiguous_but_well_separated_hits_are_not() {
        let front = occt::Shape::cube(1.0).unwrap();
        let close = occt::Shape::cube(1.0)
            .unwrap()
            .translated(DVec3::new(0.0, 0.0, -0.1))
            .unwrap();
        let far = occt::Shape::cube(1.0)
            .unwrap()
            .translated(DVec3::new(0.0, 0.0, -5.0))
            .unwrap();
        let front_mesh = tessellate(&front, 0.1);
        let close_mesh = tessellate(&close, 0.1);
        let far_mesh = tessellate(&far, 0.1);
        let make_candidates = |other_shape: &occt::Shape, other_mesh: &BodyMesh| {
            let bodies = [
                PickBody {
                    id: BodyId(1),
                    mesh: &front_mesh,
                    shape: &front,
                    pose: glam::Mat4::IDENTITY,
                },
                PickBody {
                    id: BodyId(2),
                    mesh: other_mesh,
                    shape: other_shape,
                    pose: glam::Mat4::IDENTITY,
                },
            ];
            nearest_face_per_body(&pick_all(&bodies, Vec3::new(0.5, 0.5, 2.0), Vec3::NEG_Z))
                .into_iter()
                .map(|hit| PickCandidate {
                    item: SelItem::Face(hit.body, hit.face),
                    t: hit.t,
                })
                .collect::<Vec<_>>()
        };

        let close_candidates = make_candidates(&close, &close_mesh);
        assert_eq!(ambiguous_candidates(&close_candidates, 2.0, false).len(), 2);
        let far_candidates = make_candidates(&far, &far_mesh);
        assert_eq!(ambiguous_candidates(&far_candidates, 6.2, false).len(), 1);
    }

    #[test]
    fn command_click_resolves_the_second_nearest_candidate() {
        let candidates = [
            PickCandidate {
                item: SelItem::Face(BodyId(1), 3),
                t: 2.0,
            },
            PickCandidate {
                item: SelItem::Face(BodyId(2), 7),
                t: 4.0,
            },
        ];
        assert_eq!(
            resolve_selection_candidate(&candidates, true).map(|candidate| candidate.item),
            Some(SelItem::Face(BodyId(2), 7))
        );
        assert_eq!(
            resolve_selection_candidate(&candidates, false).map(|candidate| candidate.item),
            Some(SelItem::Face(BodyId(1), 3))
        );
    }

    #[test]
    fn digit_during_supported_drag_transitions_to_frozen_input() {
        let event = KeyDownEvent {
            keystroke: Keystroke {
                key: "7".to_string(),
                key_char: Some("7".to_string()),
                ..Default::default()
            },
            is_held: false,
            prefer_character_input: false,
        };
        assert_eq!(
            Viewport::numeric_drag_transition(true, false, &event),
            NumericDragTransition::Freeze('7')
        );
        assert_eq!(
            Viewport::numeric_drag_transition(false, false, &event),
            NumericDragTransition::Ignore
        );
    }

    #[test]
    fn keyboard_xyz_resolves_world_axes_and_planes() {
        for (key, expected) in [
            ("x", glam::DVec3::X),
            ("Y", glam::DVec3::Y),
            ("z", glam::DVec3::Z),
        ] {
            let Some(ReferenceGeometry::Axis(axis)) = world_reference(key, ReferenceKind::Axis)
            else {
                panic!("axis reference for {key}");
            };
            assert_eq!(axis.origin, glam::DVec3::ZERO);
            assert_eq!(axis.direction, expected);

            let Some(ReferenceGeometry::Plane(plane)) = world_reference(key, ReferenceKind::Plane)
            else {
                panic!("plane reference for {key}");
            };
            assert_eq!(plane.origin, glam::DVec3::ZERO);
            assert_eq!(plane.normal, expected);
        }
        assert!(world_reference("q", ReferenceKind::Axis).is_none());
    }

    #[test]
    fn exploded_factor_scales_radial_offset_and_clamps() {
        let center = Vec3::new(4.0, 2.0, 0.0);
        let assembly = Vec3::new(2.0, 2.0, 0.0);
        assert_eq!(exploded_offset(center, assembly, 0.0), Vec3::ZERO);
        assert_eq!(
            exploded_offset(center, assembly, 0.5),
            Vec3::new(1.5, 0.0, 0.0)
        );
        assert_eq!(
            exploded_offset(center, assembly, 2.0),
            Vec3::new(3.0, 0.0, 0.0)
        );
    }
}
