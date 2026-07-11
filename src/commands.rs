//! Application command vocabulary dispatched by the root view.
//!
//! The floating chrome never mutates the viewport or document directly; it
//! emits [`AppCommand`]s that [`crate::app::Free3dApp`] interprets. Modeling
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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Construction => "构造",
            Self::Horizontal => "水平",
            Self::Vertical => "垂直",
            Self::Parallel => "平行",
            Self::Perpendicular => "正交",
            Self::Equal => "相等",
            Self::Tangent => "相切",
            Self::Collinear => "共线",
            Self::G2 => "曲率连续",
            Self::Fix => "锁定/解锁",
            Self::Concentric => "同心",
            Self::Coincident => "重合",
            Self::Symmetric => "对称",
            Self::PointOnObject => "点在线上",
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
    pub const fn tooltip(self) -> &'static str {
        match self {
            Self::Symmetric => "对称：先选两个含端点实体，最后选择镜像线；使用两实体最近端点",
            Self::PointOnObject => "点在线上：先选含点实体，再选直线、圆或圆弧",
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
            StandardView::Iso => "等轴测",
            StandardView::Front => "前视",
            StandardView::Back => "后视",
            StandardView::Top => "顶视",
            StandardView::Bottom => "底视",
            StandardView::Right => "右视",
            StandardView::Left => "左视",
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
            ModeChip::Section => "剖切视图",
            ModeChip::Isolate => "隔离",
            ModeChip::Measure => "测量",
            ModeChip::Exploded => "爆炸",
        }
    }

    /// Secondary state line shown under the chip, reflecting `active`.
    pub fn state_label(self, active: bool) -> &'static str {
        if active { "开启" } else { "关闭" }
    }

    /// Icon asset name for the chip.
    pub fn icon(self) -> &'static str {
        match self {
            ModeChip::Section => "section",
            ModeChip::Isolate => "isolate",
            ModeChip::Measure => "measure",
            ModeChip::Exploded => "move",
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
            ToolGroup::Sketch => "草图",
            ToolGroup::Add => "添加",
            ToolGroup::Transform => "变换",
            ToolGroup::Tools => "工具",
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
}

impl ToolId {
    /// Every modeling tool in tool-strip order.
    pub const ALL: [Self; 68] = [
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
    ];

    /// Human-readable tool name.
    pub fn label(self) -> &'static str {
        match self {
            ToolId::Line => "直线",
            ToolId::Rectangle => "矩形",
            ToolId::CenterRectangle => "中心矩形",
            ToolId::RoundedRectangle => "圆角矩形",
            ToolId::Polygon => "多边形",
            ToolId::Slot => "槽",
            ToolId::Circle => "圆",
            ToolId::ThreePointCircle => "三点圆",
            ToolId::Ellipse => "椭圆",
            ToolId::EllipseArc => "椭圆弧",
            ToolId::Arc => "圆弧",
            ToolId::Point => "点",
            ToolId::TangentArc => "切线弧",
            ToolId::Spline => "样条",
            ToolId::CvSpline => "控制点样条",
            ToolId::TwoTangentCircle => "两切线圆",
            ToolId::ThreeTangentCircle => "三切线圆",
            ToolId::SketchFillet => "草图圆角",
            ToolId::Trim => "修剪",
            ToolId::Extend => "延伸",
            ToolId::Break => "打断",
            ToolId::SketchOffset => "草图偏移",
            ToolId::Box => "长方体",
            ToolId::Cylinder => "圆柱体",
            ToolId::Sphere => "球体",
            ToolId::Cone => "圆锥体",
            ToolId::Torus => "圆环体",
            ToolId::Ellipsoid => "椭球体",
            ToolId::Prism => "棱柱",
            ToolId::Wedge => "楔形",
            ToolId::Plane => "构造平面",
            ToolId::Axis => "构造轴",
            ToolId::DatumPoint => "构造点",
            ToolId::ReferenceImage => "参考图像",
            ToolId::Helix => "螺旋线",
            ToolId::Thread => "螺纹",
            ToolId::Move => "移动/旋转",
            ToolId::Translate => "平移",
            ToolId::Scale => "缩放",
            ToolId::Mirror => "镜像",
            ToolId::Pattern => "阵列",
            ToolId::Align => "对齐",
            ToolId::Ground => "接地",
            ToolId::Joint => "关节",
            ToolId::Drive => "驱动",
            ToolId::Extrude => "拉伸",
            ToolId::Revolve => "旋转体",
            ToolId::Sweep => "扫掠",
            ToolId::Loft => "放样",
            ToolId::Patch => "修补",
            ToolId::Stitch => "缝合",
            ToolId::Thicken => "加厚",
            ToolId::DeleteFace => "删除面",
            ToolId::Shell => "抽壳",
            ToolId::Fillet => "圆角",
            ToolId::Chamfer => "倒角",
            ToolId::OffsetFace => "偏移面",
            ToolId::ReplaceFace => "替换面",
            ToolId::Hole => "孔",
            ToolId::Draft => "拔模",
            ToolId::Split => "分割体",
            ToolId::Project => "投影",
            ToolId::Union => "布尔并集",
            ToolId::Subtract => "布尔减集",
            ToolId::Intersect => "布尔交集",
            ToolId::Properties => "属性",
            ToolId::InterferenceCheck => "干涉检查",
            ToolId::GeometryCheck => "检查几何",
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
            ToolId::ReplaceFace => "offset",
            ToolId::Hole => "circle",
            ToolId::Draft => "offset",
            ToolId::Split => "split",
            ToolId::Project => "project",
            ToolId::Union => "union",
            ToolId::Subtract => "subtract",
            ToolId::Intersect => "intersect",
            ToolId::Properties => "measure",
            ToolId::InterferenceCheck => "intersect",
            ToolId::GeometryCheck => "measure",
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
            Self::Undo => "撤销",
            Self::Redo => "重做",
            Self::SaveProject => "保存",
            Self::SaveProjectAs => "另存为",
            Self::OpenProject => "打开",
            Self::NewProject => "新建项目",
            Self::Import => "导入",
            Self::Export => "导出",
            Self::Variables => "变量",
            Self::Visualize => "可视化",
            Self::Materials => "材质",
            Self::StandardView(StandardView::Iso) => "等轴测视图",
            Self::StandardView(StandardView::Front) => "前视图",
            Self::StandardView(StandardView::Back) => "后视图",
            Self::StandardView(StandardView::Top) => "顶视图",
            Self::StandardView(StandardView::Bottom) => "底视图",
            Self::StandardView(StandardView::Right) => "右视图",
            Self::StandardView(StandardView::Left) => "左视图",
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
