//! Application command vocabulary dispatched by the root view.
//!
//! The floating chrome never mutates the viewport or document directly; it
//! emits [`AppCommand`]s that [`crate::app::DuctileApp`] interprets. Modeling
//! tools are stubs at this milestone: activating one only tracks selection
//! state so the UI can render active highlights.

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

/// A single user intent produced by the chrome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppCommand {
    /// Undo the last document mutation (stub).
    Undo,
    /// Redo the last undone mutation (stub).
    Redo,
    /// Save the native project, prompting for a path when needed.
    SaveProject,
    /// Save the native project to a newly chosen path.
    SaveProjectAs,
    /// Open a native project from disk.
    OpenProject,
    /// Replace the current document with an empty project.
    NewProject,
    /// Import geometry from disk (stub).
    Import,
    /// Export the document (stub).
    Export,
    /// Open the settings surface (stub).
    OpenSettings,
    /// Focus the command search field (stub).
    CommandSearch,
    /// Make `tool` the active modeling tool.
    ActivateTool(ToolId),
    /// Snap the camera to a standard orientation.
    StandardView(StandardView),
    /// Toggle the Items panel visibility.
    ToggleItemsPanel,
    /// Toggle the History panel visibility.
    ToggleHistoryPanel,
    /// Toggle the Variables panel visibility.
    ToggleVariablesPanel,
    /// Toggle a bottom-left interaction mode.
    ToggleMode(ModeChip),
    /// Toggle grid plane visibility in the viewport.
    ToggleGrid,
    /// Toggle the snap magnet (stub state only).
    ToggleSnap,
    /// Toggle shaded/wireframe viewport display.
    ToggleWireframe,
    /// Save the current viewport frame to a PNG on the desktop.
    Screenshot,
    /// Enter the material-focused Visualize workspace.
    VisualizeSpace,
}

/// Constraint action exposed by the sketch-mode constraints panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SketchConstraintKind {
    Construction,
    Horizontal,
    Vertical,
    Parallel,
    Perpendicular,
    Equal,
    Tangent,
    Collinear,
    G2,
    Fix,
    Concentric,
    Coincident,
    Symmetric,
    PointOnObject,
}

impl SketchConstraintKind {
    /// Panel order, matching the common single-entity actions first.
    pub const ALL: [Self; 14] = [
        Self::Construction,
        Self::Horizontal,
        Self::Vertical,
        Self::Parallel,
        Self::Perpendicular,
        Self::Equal,
        Self::Tangent,
        Self::Collinear,
        Self::G2,
        Self::Fix,
        Self::Concentric,
        Self::Coincident,
        Self::Symmetric,
        Self::PointOnObject,
    ];

    /// Compact panel label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Construction => crate::i18n::t("Construction"),
            Self::Horizontal => crate::i18n::t("Horizontal"),
            Self::Vertical => crate::i18n::t("Vertical"),
            Self::Parallel => crate::i18n::t("Parallel"),
            Self::Perpendicular => crate::i18n::t("Perpendicular"),
            Self::Equal => crate::i18n::t("Equal"),
            Self::Tangent => crate::i18n::t("Tangent"),
            Self::Collinear => crate::i18n::t("Collinear"),
            Self::G2 => crate::i18n::t("Curvature Continuous"),
            Self::Fix => crate::i18n::t("Lock/Unlock"),
            Self::Concentric => crate::i18n::t("Concentric"),
            Self::Coincident => crate::i18n::t("Coincident"),
            Self::Symmetric => crate::i18n::t("Symmetric"),
            Self::PointOnObject => crate::i18n::t("Point on Object"),
        }
    }

    /// Short original glyph used in the narrow vertical panel.
    pub const fn mark(self) -> &'static str {
        match self {
            Self::Construction => "◇",
            Self::Horizontal => "H",
            Self::Vertical => "V",
            Self::Parallel => "∥",
            Self::Perpendicular => "⊥",
            Self::Equal => "=",
            Self::Tangent => "T",
            Self::Collinear => "≡",
            Self::G2 => "G²",
            Self::Fix => "🔒",
            Self::Concentric => "◎",
            Self::Coincident => "•",
            Self::Symmetric => "⇋",
            Self::PointOnObject => "⊙",
        }
    }

    /// Full tooltip, including selection-order rules where they matter.
    pub fn tooltip(self) -> &'static str {
        match self {
            Self::Symmetric => crate::i18n::t(
                "Symmetric: select two endpoint-bearing entities, then the mirror line; the nearest endpoints are used",
            ),
            Self::PointOnObject => crate::i18n::t(
                "Point on Object: select a point-bearing entity, then a line, circle, or arc",
            ),
            _ => self.label(),
        }
    }
}

/// A standard camera orientation reachable from the view cluster or `Cmd+1..7`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StandardView {
    /// Default front-right-top isometric.
    Iso,
    Front,
    Back,
    Top,
    Bottom,
    Right,
    Left,
}

impl StandardView {
    /// Every standard view, in view-cluster button order.
    pub const ALL: [StandardView; 7] = [
        StandardView::Iso,
        StandardView::Front,
        StandardView::Back,
        StandardView::Top,
        StandardView::Bottom,
        StandardView::Right,
        StandardView::Left,
    ];

    /// Short label rendered on the standard-view buttons.
    pub fn label(self) -> &'static str {
        match self {
            StandardView::Iso => crate::i18n::t("Isometric"),
            StandardView::Front => crate::i18n::t("Front View"),
            StandardView::Back => crate::i18n::t("Back View"),
            StandardView::Top => crate::i18n::t("Top View"),
            StandardView::Bottom => crate::i18n::t("Bottom View"),
            StandardView::Right => crate::i18n::t("Right View"),
            StandardView::Left => crate::i18n::t("Left View"),
        }
    }

    /// Target `(yaw, pitch)` orbit angles in radians for this view.
    pub fn orientation(self) -> (f32, f32) {
        // Kept in lock-step with the historical `Cmd+1..7` key mapping.
        const NEAR_POLE: f32 = FRAC_PI_2 - 0.001_75;
        match self {
            StandardView::Iso => (-3.0 * FRAC_PI_4, -0.55),
            StandardView::Front => (FRAC_PI_2, 0.0),
            StandardView::Back => (-FRAC_PI_2, 0.0),
            StandardView::Top => (0.0, -NEAR_POLE),
            StandardView::Bottom => (0.0, NEAR_POLE),
            StandardView::Right => (PI, 0.0),
            StandardView::Left => (0.0, 0.0),
        }
    }

    /// Maps a bare `Cmd+<digit>` key to a standard view.
    pub fn from_digit(key: &str) -> Option<StandardView> {
        Some(match key {
            "1" => StandardView::Iso,
            "2" => StandardView::Front,
            "3" => StandardView::Back,
            "4" => StandardView::Top,
            "5" => StandardView::Bottom,
            "6" => StandardView::Right,
            "7" => StandardView::Left,
            _ => return None,
        })
    }
}

/// A bottom-left interaction mode chip (all stubs this milestone).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeChip {
    Section,
    Isolate,
    Measure,
    Exploded,
}

impl ModeChip {
    /// Every mode chip, in display order.
    pub const ALL: [ModeChip; 4] = [
        ModeChip::Section,
        ModeChip::Isolate,
        ModeChip::Measure,
        ModeChip::Exploded,
    ];

    /// Chip label.
    pub fn label(self) -> &'static str {
        match self {
            ModeChip::Section => crate::i18n::t("Section View Mode"),
            ModeChip::Isolate => crate::i18n::t("Isolate"),
            ModeChip::Measure => crate::i18n::t("Measure"),
            ModeChip::Exploded => crate::i18n::t("Exploded"),
        }
    }

    /// Icon asset name for the chip.
    pub fn icon(self) -> &'static str {
        match self {
            ModeChip::Section => "section",
            ModeChip::Isolate => "isolate",
            ModeChip::Measure => "measure",
            ModeChip::Exploded => "exploded",
        }
    }
}

/// A modeling tool group shown as a top-level button on the tool strip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolGroup {
    Sketch,
    Add,
    Transform,
    Tools,
}

impl ToolGroup {
    /// Every group, top-to-bottom on the strip.
    pub const ALL: [ToolGroup; 4] = [
        ToolGroup::Sketch,
        ToolGroup::Add,
        ToolGroup::Transform,
        ToolGroup::Tools,
    ];

    /// Group label shown in tooltips and flyout headers.
    pub fn label(self) -> &'static str {
        match self {
            ToolGroup::Sketch => crate::i18n::t("Sketch"),
            ToolGroup::Add => crate::i18n::t("Add"),
            ToolGroup::Transform => crate::i18n::t("Transform"),
            ToolGroup::Tools => crate::i18n::t("Tools"),
        }
    }

    /// Icon asset name for the group button.
    pub fn icon(self) -> &'static str {
        match self {
            ToolGroup::Sketch => "sketch",
            ToolGroup::Add => "add",
            ToolGroup::Transform => "transform",
            ToolGroup::Tools => "modify",
        }
    }

    /// Tools belonging to this group, in flyout order.
    pub fn tools(self) -> &'static [ToolId] {
        match self {
            ToolGroup::Sketch => &[
                ToolId::Line,
                ToolId::Rectangle,
                ToolId::CenterRectangle,
                ToolId::RoundedRectangle,
                ToolId::Polygon,
                ToolId::Slot,
                ToolId::Circle,
                ToolId::ThreePointCircle,
                ToolId::Ellipse,
                ToolId::EllipseArc,
                ToolId::Arc,
                ToolId::Point,
                ToolId::TangentArc,
                ToolId::Spline,
                ToolId::CvSpline,
                ToolId::TwoTangentCircle,
                ToolId::ThreeTangentCircle,
                ToolId::SketchFillet,
                ToolId::Trim,
                ToolId::Extend,
                ToolId::Break,
                ToolId::SketchOffset,
            ],
            ToolGroup::Add => &[
                ToolId::Box,
                ToolId::Cylinder,
                ToolId::Sphere,
                ToolId::Cone,
                ToolId::Torus,
                ToolId::Ellipsoid,
                ToolId::Prism,
                ToolId::Wedge,
                ToolId::Plane,
                ToolId::Axis,
                ToolId::DatumPoint,
                ToolId::ReferenceImage,
                ToolId::Helix,
                ToolId::Thread,
            ],
            ToolGroup::Transform => &[
                ToolId::Move,
                ToolId::Translate,
                ToolId::Scale,
                ToolId::Mirror,
                ToolId::Pattern,
                ToolId::Align,
                ToolId::Ground,
                ToolId::Joint,
                ToolId::Drive,
            ],
            ToolGroup::Tools => &[
                ToolId::Extrude,
                ToolId::Revolve,
                ToolId::Sweep,
                ToolId::Loft,
                ToolId::Patch,
                ToolId::Stitch,
                ToolId::Thicken,
                ToolId::DeleteFace,
                ToolId::Shell,
                ToolId::Fillet,
                ToolId::Chamfer,
                ToolId::OffsetFace,
                ToolId::ReplaceFace,
                ToolId::Hole,
                ToolId::Draft,
                ToolId::Split,
                ToolId::Project,
                ToolId::Union,
                ToolId::Subtract,
                ToolId::Intersect,
                ToolId::Properties,
                ToolId::InterferenceCheck,
                ToolId::GeometryCheck,
            ],
        }
    }

    /// Whether `tool` lives in this group.
    pub fn contains(self, tool: ToolId) -> bool {
        self.tools().contains(&tool)
    }
}

/// An individual modeling tool. All tools are stubs this milestone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolId {
    // Sketch
    Line,
    Rectangle,
    CenterRectangle,
    RoundedRectangle,
    Polygon,
    Slot,
    Circle,
    ThreePointCircle,
    Ellipse,
    EllipseArc,
    Arc,
    Point,
    TangentArc,
    Spline,
    CvSpline,
    TwoTangentCircle,
    ThreeTangentCircle,
    SketchFillet,
    Trim,
    Extend,
    Break,
    SketchOffset,
    // Add
    Box,
    Cylinder,
    Sphere,
    Cone,
    Torus,
    Ellipsoid,
    Prism,
    Wedge,
    Plane,
    Axis,
    DatumPoint,
    ReferenceImage,
    Helix,
    Thread,
    // Transform
    Move,
    Translate,
    Scale,
    Mirror,
    Pattern,
    Align,
    Ground,
    Joint,
    Drive,
    // Modify
    Extrude,
    Revolve,
    Sweep,
    Loft,
    Patch,
    Stitch,
    Thicken,
    DeleteFace,
    Shell,
    Fillet,
    Chamfer,
    OffsetFace,
    ReplaceFace,
    Hole,
    Draft,
    Split,
    Project,
    Union,
    Subtract,
    Intersect,
    // Inspect
    Properties,
    InterferenceCheck,
    GeometryCheck,
    /// Point/edge/face measurement mode.
    Measure,
}

impl ToolId {
    /// Every modeling tool in tool-strip order.
    pub const ALL: [Self; 69] = [
        Self::Line,
        Self::Rectangle,
        Self::CenterRectangle,
        Self::RoundedRectangle,
        Self::Polygon,
        Self::Slot,
        Self::Circle,
        Self::ThreePointCircle,
        Self::Ellipse,
        Self::EllipseArc,
        Self::Arc,
        Self::Point,
        Self::TangentArc,
        Self::Spline,
        Self::CvSpline,
        Self::TwoTangentCircle,
        Self::ThreeTangentCircle,
        Self::SketchFillet,
        Self::Trim,
        Self::Extend,
        Self::Break,
        Self::SketchOffset,
        Self::Box,
        Self::Cylinder,
        Self::Sphere,
        Self::Cone,
        Self::Torus,
        Self::Ellipsoid,
        Self::Prism,
        Self::Wedge,
        Self::Plane,
        Self::Axis,
        Self::DatumPoint,
        Self::ReferenceImage,
        Self::Helix,
        Self::Thread,
        Self::Move,
        Self::Translate,
        Self::Scale,
        Self::Mirror,
        Self::Pattern,
        Self::Align,
        Self::Ground,
        Self::Joint,
        Self::Drive,
        Self::Extrude,
        Self::Revolve,
        Self::Sweep,
        Self::Loft,
        Self::Patch,
        Self::Stitch,
        Self::Thicken,
        Self::DeleteFace,
        Self::Shell,
        Self::Fillet,
        Self::Chamfer,
        Self::OffsetFace,
        Self::ReplaceFace,
        Self::Hole,
        Self::Draft,
        Self::Split,
        Self::Project,
        Self::Union,
        Self::Subtract,
        Self::Intersect,
        Self::Properties,
        Self::InterferenceCheck,
        Self::GeometryCheck,
        Self::Measure,
    ];

    /// Human-readable tool name.
    pub fn label(self) -> &'static str {
        match self {
            ToolId::Line => crate::i18n::t("Line"),
            ToolId::Rectangle => crate::i18n::t("Rectangle"),
            ToolId::CenterRectangle => crate::i18n::t("Center Rectangle"),
            ToolId::RoundedRectangle => crate::i18n::t("Rounded Rectangle"),
            ToolId::Polygon => crate::i18n::t("Polygon"),
            ToolId::Slot => crate::i18n::t("Slot"),
            ToolId::Circle => crate::i18n::t("Circle"),
            ToolId::ThreePointCircle => crate::i18n::t("Three-Point Circle"),
            ToolId::Ellipse => crate::i18n::t("Ellipse"),
            ToolId::EllipseArc => crate::i18n::t("Elliptical Arc"),
            ToolId::Arc => crate::i18n::t("Arc"),
            ToolId::Point => crate::i18n::t("Point"),
            ToolId::TangentArc => crate::i18n::t("Tangent Arc"),
            ToolId::Spline => crate::i18n::t("Spline"),
            ToolId::CvSpline => crate::i18n::t("Control-Point Spline"),
            ToolId::TwoTangentCircle => crate::i18n::t("Two-Tangent Circle"),
            ToolId::ThreeTangentCircle => crate::i18n::t("Three-Tangent Circle"),
            ToolId::SketchFillet => crate::i18n::t("Sketch Fillet"),
            ToolId::Trim => crate::i18n::t("Trim"),
            ToolId::Extend => crate::i18n::t("Extend"),
            ToolId::Break => crate::i18n::t("Break"),
            ToolId::SketchOffset => crate::i18n::t("Sketch Offset"),
            ToolId::Box => crate::i18n::t("Box"),
            ToolId::Cylinder => crate::i18n::t("Cylinder"),
            ToolId::Sphere => crate::i18n::t("Sphere"),
            ToolId::Cone => crate::i18n::t("Cone"),
            ToolId::Torus => crate::i18n::t("Torus"),
            ToolId::Ellipsoid => crate::i18n::t("Ellipsoid"),
            ToolId::Prism => crate::i18n::t("Prism"),
            ToolId::Wedge => crate::i18n::t("Wedge"),
            ToolId::Plane => crate::i18n::t("Construction Plane"),
            ToolId::Axis => crate::i18n::t("Construction Axis"),
            ToolId::DatumPoint => crate::i18n::t("Construction Point"),
            ToolId::ReferenceImage => crate::i18n::t("Reference Image"),
            ToolId::Helix => crate::i18n::t("Helix"),
            ToolId::Thread => crate::i18n::t("Thread"),
            ToolId::Move => crate::i18n::t("Move/Rotate"),
            ToolId::Translate => crate::i18n::t("Translate"),
            ToolId::Scale => crate::i18n::t("Scale"),
            ToolId::Mirror => crate::i18n::t("Mirror"),
            ToolId::Pattern => crate::i18n::t("Pattern"),
            ToolId::Align => crate::i18n::t("Align"),
            ToolId::Ground => crate::i18n::t("Ground"),
            ToolId::Joint => crate::i18n::t("Joint"),
            ToolId::Drive => crate::i18n::t("Drive"),
            ToolId::Extrude => crate::i18n::t("Extrude"),
            ToolId::Revolve => crate::i18n::t("Revolve"),
            ToolId::Sweep => crate::i18n::t("Sweep"),
            ToolId::Loft => crate::i18n::t("Loft"),
            ToolId::Patch => crate::i18n::t("Patch"),
            ToolId::Stitch => crate::i18n::t("Stitch"),
            ToolId::Thicken => crate::i18n::t("Thicken"),
            ToolId::DeleteFace => crate::i18n::t("Delete Face"),
            ToolId::Shell => crate::i18n::t("Shell"),
            ToolId::Fillet => crate::i18n::t("Fillet"),
            ToolId::Chamfer => crate::i18n::t("Chamfer"),
            ToolId::OffsetFace => crate::i18n::t("Offset Face"),
            ToolId::ReplaceFace => crate::i18n::t("Replace Face"),
            ToolId::Hole => crate::i18n::t("Hole"),
            ToolId::Draft => crate::i18n::t("Draft"),
            ToolId::Split => crate::i18n::t("Split Body"),
            ToolId::Project => crate::i18n::t("Project Geometry"),
            ToolId::Union => crate::i18n::t("Boolean Union"),
            ToolId::Subtract => crate::i18n::t("Boolean Subtract"),
            ToolId::Intersect => crate::i18n::t("Boolean Intersect"),
            ToolId::Properties => crate::i18n::t("Properties"),
            ToolId::InterferenceCheck => crate::i18n::t("Interference Check"),
            ToolId::GeometryCheck => crate::i18n::t("Check Geometry"),
            ToolId::Measure => crate::i18n::t("Measure"),
        }
    }

    /// Keyboard shortcut label, if any.
    pub fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            ToolId::Line => "L",
            ToolId::Rectangle => "R",
            ToolId::Circle => "C",
            ToolId::TangentArc => "T",
            ToolId::Arc => "A",
            ToolId::Box => "B",
            ToolId::Cylinder => "Y",
            ToolId::Sphere => "S",
            ToolId::Move => "M",
            ToolId::Extrude => "E",
            ToolId::Revolve => "V",
            ToolId::Fillet => "F",
            ToolId::Mirror => "I",
            ToolId::Union => "U",
            _ => return None,
        })
    }

    /// Icon asset name (under `icons/`) for this tool.
    pub fn icon(self) -> &'static str {
        match self {
            ToolId::Line => "line",
            ToolId::Rectangle => "rectangle",
            ToolId::CenterRectangle => "center-rectangle",
            ToolId::RoundedRectangle => "rounded-rectangle",
            ToolId::Polygon => "polygon",
            ToolId::Slot => "slot",
            ToolId::Circle => "circle",
            ToolId::ThreePointCircle => "three-point-circle",
            ToolId::Ellipse => "ellipse",
            ToolId::EllipseArc => "ellipse-arc",
            ToolId::Arc => "arc",
            ToolId::Point => "point",
            ToolId::TangentArc => "tangent-arc",
            ToolId::Spline => "spline",
            ToolId::CvSpline => "spline",
            ToolId::TwoTangentCircle | ToolId::ThreeTangentCircle => "circle",
            ToolId::SketchFillet => "sketch-fillet",
            ToolId::Trim => "trim",
            ToolId::Extend => "trim",
            ToolId::Break => "split",
            ToolId::SketchOffset => "sketch-offset",
            ToolId::Box => "box",
            ToolId::Cylinder => "cylinder",
            ToolId::Sphere => "sphere",
            ToolId::Cone => "cone",
            ToolId::Torus => "torus",
            ToolId::Ellipsoid => "sphere",
            ToolId::Prism => "polygon",
            ToolId::Wedge => "box",
            ToolId::Plane => "plane",
            ToolId::Axis => "axis",
            ToolId::DatumPoint => "point",
            ToolId::ReferenceImage => "image",
            ToolId::Helix => "sweep",
            ToolId::Thread => "sweep",
            ToolId::Move => "move",
            ToolId::Translate => "translate",
            ToolId::Scale => "scale",
            ToolId::Mirror => "mirror",
            ToolId::Pattern => "pattern",
            ToolId::Align => "align",
            ToolId::Ground => "axis",
            ToolId::Joint | ToolId::Drive => "move",
            ToolId::Extrude => "extrude",
            ToolId::Revolve => "revolve",
            ToolId::Sweep => "sweep",
            ToolId::Loft => "loft",
            ToolId::Patch => "patch",
            ToolId::Stitch => "stitch",
            ToolId::Thicken => "thicken",
            ToolId::DeleteFace => "delete-face",
            ToolId::Shell => "shell",
            ToolId::Fillet => "fillet",
            ToolId::Chamfer => "fillet",
            ToolId::OffsetFace => "offset",
            ToolId::ReplaceFace => "replace-face",
            ToolId::Hole => "circle",
            ToolId::Draft => "draft",
            ToolId::Split => "split",
            ToolId::Project => "project",
            ToolId::Union => "union",
            ToolId::Subtract => "subtract",
            ToolId::Intersect => "intersect",
            ToolId::Properties => "measure",
            ToolId::InterferenceCheck => "intersect",
            ToolId::GeometryCheck => "measure",
            ToolId::Measure => "measure",
        }
    }
}

/// An entry in the command-search registry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchCommand {
    /// A modeling tool command.
    Tool(ToolId),
    Undo,
    Redo,
    SaveProject,
    SaveProjectAs,
    OpenProject,
    NewProject,
    Import,
    Export,
    Variables,
    Visualize,
    Materials,
    StandardView(StandardView),
}

impl SearchCommand {
    /// Builds the complete searchable command registry.
    pub fn all() -> Vec<Self> {
        ToolId::ALL
            .into_iter()
            .map(Self::Tool)
            .chain([
                Self::Undo,
                Self::Redo,
                Self::SaveProject,
                Self::SaveProjectAs,
                Self::OpenProject,
                Self::NewProject,
                Self::Import,
                Self::Export,
                Self::Variables,
                Self::Visualize,
                Self::Materials,
            ])
            .chain(StandardView::ALL.into_iter().map(Self::StandardView))
            .collect()
    }

    /// Converts this registry entry to the root command it invokes.
    pub const fn app_command(self) -> AppCommand {
        match self {
            Self::Tool(tool) => AppCommand::ActivateTool(tool),
            Self::Undo => AppCommand::Undo,
            Self::Redo => AppCommand::Redo,
            Self::SaveProject => AppCommand::SaveProject,
            Self::SaveProjectAs => AppCommand::SaveProjectAs,
            Self::OpenProject => AppCommand::OpenProject,
            Self::NewProject => AppCommand::NewProject,
            Self::Import => AppCommand::Import,
            Self::Export => AppCommand::Export,
            Self::Variables => AppCommand::ToggleVariablesPanel,
            Self::Visualize | Self::Materials => AppCommand::VisualizeSpace,
            Self::StandardView(view) => AppCommand::StandardView(view),
        }
    }

    /// User-facing command label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Tool(tool) => tool.label(),
            Self::Undo => crate::i18n::t("Undo"),
            Self::Redo => crate::i18n::t("Redo"),
            Self::SaveProject => crate::i18n::t("Save"),
            Self::SaveProjectAs => crate::i18n::t("Save As"),
            Self::OpenProject => crate::i18n::t("Open"),
            Self::NewProject => crate::i18n::t("New Project"),
            Self::Import => crate::i18n::t("Import"),
            Self::Export => crate::i18n::t("Export"),
            Self::Variables => crate::i18n::t("Variables"),
            Self::Visualize => crate::i18n::t("Visualize"),
            Self::Materials => crate::i18n::t("Materials"),
            Self::StandardView(StandardView::Iso) => crate::i18n::t("Isometric View"),
            Self::StandardView(StandardView::Front) => crate::i18n::t("Front View"),
            Self::StandardView(StandardView::Back) => crate::i18n::t("Back View"),
            Self::StandardView(StandardView::Top) => crate::i18n::t("Top View"),
            Self::StandardView(StandardView::Bottom) => crate::i18n::t("Bottom View"),
            Self::StandardView(StandardView::Right) => crate::i18n::t("Right View"),
            Self::StandardView(StandardView::Left) => crate::i18n::t("Left View"),
        }
    }

    /// Icon asset name.
    pub fn icon(self) -> &'static str {
        match self {
            Self::Tool(tool) => tool.icon(),
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::SaveProject | Self::SaveProjectAs => "export",
            Self::OpenProject | Self::NewProject => "home",
            Self::Import => "import",
            Self::Export => "export",
            Self::Variables => "items",
            Self::Visualize | Self::Materials => "visualize",
            Self::StandardView(_) => "views",
        }
    }

    /// Shortcut chip text, when the command has a shortcut.
    pub fn shortcut(self) -> Option<&'static str> {
        match self {
            Self::Tool(tool) => tool.shortcut(),
            Self::Undo => Some("Cmd Z"),
            Self::Redo => Some("Cmd Shift Z"),
            Self::SaveProject => Some("Cmd S"),
            Self::SaveProjectAs => Some("Cmd Shift S"),
            Self::OpenProject => Some("Cmd O"),
            Self::NewProject => Some("Cmd N"),
            Self::StandardView(StandardView::Iso) => Some("Cmd 1"),
            Self::StandardView(StandardView::Front) => Some("Cmd 2"),
            Self::StandardView(StandardView::Back) => Some("Cmd 3"),
            Self::StandardView(StandardView::Top) => Some("Cmd 4"),
            Self::StandardView(StandardView::Bottom) => Some("Cmd 5"),
            Self::StandardView(StandardView::Right) => Some("Cmd 6"),
            Self::StandardView(StandardView::Left) => Some("Cmd 7"),
            Self::Variables => Some("Cmd Alt V"),
            Self::Import | Self::Export | Self::Visualize | Self::Materials => None,
        }
    }
}
