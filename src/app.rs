//! Application root: owns the viewport and the floating chrome state, and
//! interprets the [`AppCommand`]s the chrome emits.

use std::{collections::HashMap, sync::Arc, time::Duration};

use glam::{DVec2, DVec3, dvec3};
use gpui::{
    AppContext, Context, Entity, FocusHandle, KeyDownEvent, PathPromptOptions, PromptLevel, Render,
    Subscription, Window, div, prelude::*,
};
use occt::Shape;

use crate::{
    assembly::{Connector, ConnectorFrame, ConnectorSource, Joint, JointId, JointKind},
    commands::{AppCommand, ModeChip, SearchCommand, SketchConstraintKind, ToolGroup, ToolId},
    constraint::{Constraint, EntityRef, PointRef},
    document::{
        BodyId, BooleanOp, Document, Material, SelItem, SelectionFilter, hsl_to_rgb, rgb_to_hsl,
    },
    drawing::{ProjectedView, Projection, ViewKind},
    history::{PathRef, PrimitiveKind, edge_ref},
    inspection::{AggregateProperties, Interference, aggregate_properties, find_interferences},
    nav::NavPreset,
    saved_views::SavedViews,
    sketch::{
        SketchEntity, SketchItem, SketchPlane, arc_center_radius, ellipse_point, regular_polygon,
    },
    theme::Theme,
    tools::extrude::ExtrudeSideMode,
    ui::{
        self,
        numeric_input::{NumericInput, NumericInputEvent},
    },
    units::Units,
    viewport::{AnalysisMode, DisplayMode, Viewport, ViewportEvent},
};

fn joint_editor_internal_value(kind: JointKind, units: Units, displayed: f64) -> f64 {
    if kind == JointKind::Revolute {
        displayed.to_radians()
    } else {
        units.parse_value(displayed)
    }
}

/// Top-level application workspace.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Space {
    /// Interactive 3D modeling.
    #[default]
    Modeling,
    /// Material assignment and enhanced presentation lighting.
    Visualize,
    /// A4 2D technical drawing.
    Drawing,
}

/// Full-window application destination.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AppScreen {
    /// Project design library.
    #[default]
    Home,
    /// Interactive modeling workspace.
    Workspace,
}

/// Active tool in the drawing workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DrawingTool {
    /// Place a projected model view.
    View,
    /// Create a snapped linear dimension.
    Dimension,
    /// Create a clipped section from an existing view.
    Section,
    /// Create a circular magnified detail from an existing view.
    Detail,
    /// Dimension a fitted circle radius.
    Radius,
    /// Dimension a fitted circle diameter.
    Diameter,
    /// Dimension the included angle of two lines.
    Angle,
    /// Place a live bill of materials.
    Bom,
    /// Attach an item balloon to tagged view geometry.
    Balloon,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum DrawingDrag {
    View { id: u64, grab: DVec2 },
    Dimension { index: usize },
    Detail { parent_id: u64, center: DVec2 },
    Bom { index: usize, grab: DVec2 },
    Balloon { index: usize, grab: DVec2 },
}

/// In-progress section-line selection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawingSectionState {
    pub parent_id: u64,
    pub first: Option<DVec2>,
}

/// Editable title-block field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawingTitleField {
    ProjectName,
    DrawingNumber,
    Scale,
    Date,
    Units,
    Author,
}

/// Numeric slot selected in an editable history operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HistoryNumericField {
    Primary,
    Count,
}

/// One editable custom-colour HSL component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MaterialNumericField {
    Hue,
    Saturation,
    Lightness,
}

/// Content of the dismissible read-only inspection card.
pub enum InspectionCard {
    Properties(Result<AggregateProperties, String>),
    Interference(Result<Vec<Interference>, String>),
    Validity {
        body_name: String,
        issues: Result<Vec<String>, String>,
    },
}

/// Root view: the full-window viewport with floating chrome layered above it.
pub struct Free3dApp {
    /// Active full-window destination.
    pub screen: AppScreen,
    /// Active top-level workspace.
    pub space: Space,
    /// Shared editable document, also held by the viewport.
    pub document: Entity<Document>,
    /// The 3D canvas, rendered as the base layer.
    pub viewport: Entity<Viewport>,
    /// Native project path, or none until the first save.
    pub project_path: Option<std::path::PathBuf>,
    /// Revision last successfully saved or loaded.
    pub saved_revision: u64,
    /// Most-recently used native project paths, newest first.
    pub recent_files: Vec<std::path::PathBuf>,
    /// Projects currently visible to the home library.
    pub home_designs: Vec<crate::home::DesignEntry>,
    /// Live home-library search text.
    pub home_query: String,
    /// Keyboard focus for search and inline rename.
    pub(crate) home_focus: FocusHandle,
    /// Card whose action menu is open.
    pub(crate) home_menu_path: Option<std::path::PathBuf>,
    /// Card currently being renamed.
    pub(crate) home_rename_path: Option<std::path::PathBuf>,
    /// Inline rename text.
    pub(crate) home_rename_buffer: String,
    /// Lazily decoded project previews, including cached failures.
    pub(crate) home_thumbnails: HashMap<std::path::PathBuf, Option<Arc<gpui::RenderImage>>>,
    /// Active design tokens.
    pub theme: Theme,
    /// Currently active modeling tool, if any (stub selection state).
    pub active_tool: Option<ToolId>,
    /// Open mass/interference/validity result card.
    pub inspection_card: Option<InspectionCard>,
    /// Side count used by the next regular-prism insertion.
    pub prism_sides: u32,
    /// The tool group whose flyout is open, if any.
    pub open_group: Option<ToolGroup>,
    /// Whether the Items panel is visible.
    pub show_items: bool,
    /// Whether the History panel is visible.
    pub show_history: bool,
    /// Whether the Variables panel is visible.
    pub show_variables: bool,
    /// Whether the view/appearance popover is open.
    pub show_views: bool,
    /// Whether the command-search palette is open.
    pub show_command_search: bool,
    /// Current command-search query.
    pub command_query: String,
    /// Highlighted row within the filtered command list.
    pub command_highlight: usize,
    /// Focus target for command-search keyboard input.
    pub(crate) command_focus: FocusHandle,
    /// Whether the settings popover is open.
    pub show_settings: bool,
    /// Whether the navigation-preset menu is expanded.
    pub show_nav_presets: bool,
    /// Active navigation bindings.
    pub nav_preset: NavPreset,
    /// Eight session-only saved camera views.
    pub saved_views: SavedViews,
    /// Grid plane visibility (mirrors the viewport).
    pub grid_visible: bool,
    /// Modeling grid state restored after leaving Visualize space.
    grid_before_visualize: Option<bool>,
    selection_filter_before_visualize: Option<SelectionFilter>,
    /// Body and feature-edge presentation (mirrors the viewport).
    pub display_mode: DisplayMode,
    /// Mutually exclusive surface-analysis overlay.
    pub analysis: AnalysisMode,
    /// User-facing length unit; geometry remains millimetre-based.
    pub units: Units,
    /// Persisted language preference (which may remain automatic).
    pub language: crate::i18n::LangChoice,
    /// Persisted autosave cadence in seconds; zero disables autosave.
    pub autosave_interval_secs: u64,
    /// Snap magnet toggle (stub state only; no viewport effect yet).
    pub snap_enabled: bool,
    /// Current vertical field of view, in degrees.
    pub fov_degrees: f32,
    /// Active bottom-left mode chips, indexed by [`ModeChip`] order.
    active_modes: [bool; 4],
    /// Whether the FOV slider is being scrubbed.
    fov_dragging: bool,
    /// Last pointer x during an FOV scrub, in logical pixels.
    fov_last_x: f32,
    _document_subscription: Subscription,
    _viewport_subscription: Subscription,
    pub(crate) renaming_body: Option<crate::document::BodyId>,
    pub(crate) renaming_plane: Option<crate::document::PlaneId>,
    pub(crate) renaming_reference_image: Option<crate::document::ReferenceImageId>,
    pub(crate) renaming_variable: Option<usize>,
    pub(crate) rename_buffer: String,
    rename_focus: FocusHandle,
    /// Most recently rejected panel action; cleared by the next panel action.
    pub last_constraint_conflict: Option<SketchConstraintKind>,
    /// Timeline row whose numeric editor is open.
    pub(crate) history_editor: Option<(usize, HistoryNumericField, Entity<NumericInput>)>,
    history_editor_subscription: Option<Subscription>,
    /// Variable expression row whose editor is open.
    pub(crate) variable_editor: Option<(usize, Entity<NumericInput>)>,
    variable_editor_subscription: Option<Subscription>,
    /// Active H/S/L editor in the Visualize material card.
    pub(crate) material_editor: Option<(BodyId, MaterialNumericField, Entity<NumericInput>)>,
    material_editor_subscription: Option<Subscription>,
    /// Active assembly joint numeric editor.
    pub(crate) joint_editor: Option<(JointId, Entity<NumericInput>)>,
    joint_editor_subscription: Option<Subscription>,
    /// Most recent replay failure displayed under its history row.
    pub(crate) replay_error: Option<(usize, String)>,
    pub drawing_tool: Option<DrawingTool>,
    pub drawing_pending_view_at: Option<DVec2>,
    pub drawing_pending_dim: Option<(u64, DVec2)>,
    pub(crate) drawing_pending_section: Option<DrawingSectionState>,
    pub(crate) drawing_pending_angle: Option<(u64, DVec2, DVec2)>,
    pub(crate) drawing_title_editor: Option<DrawingTitleField>,
    pub(crate) drawing_detail_scale: f64,
    pub(crate) drawing_numeric_buffer: String,
    pub drawing_selected_view: Option<u64>,
    pub(crate) drawing_selected_dim: Option<usize>,
    pub(crate) drawing_drag: Option<DrawingDrag>,
    pub(crate) drawing_cache: HashMap<u64, (u64, ProjectedView)>,
    pub(crate) drawing_bom_cache: Option<(u64, Units, Vec<crate::drawing::BomRow>)>,
    pub(crate) exploded_factor: f32,
    pub(crate) exploded_dragging: bool,
    exploded_last_x: f32,
}

impl Free3dApp {
    /// Creates the root, its viewport child and default chrome state.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cli_path = std::env::args_os()
            .skip(1)
            .map(std::path::PathBuf::from)
            .find(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("f3d"))
            });
        let demo = std::env::var_os("FREE3D_DEMO_SCENE").is_some();
        let startup_path = cli_path.filter(|path| path.is_file());
        let startup = startup_path
            .as_deref()
            .and_then(|path| Document::load_from(path).ok())
            .unwrap_or_else(startup_document);
        let document = cx.new(|_| startup);
        let settings = crate::settings::load();
        // `FREE3D_THEME=light|dark` forces the startup appearance for VM
        // verification; otherwise the default (light) theme is used.
        let theme = match std::env::var("FREE3D_THEME").ok().as_deref() {
            Some("dark") => Theme::dark(),
            Some("light") => Theme::light(),
            _ if settings.dark_theme => Theme::dark(),
            _ => Theme::default(),
        };
        let viewport =
            cx.new(|cx| Viewport::new(document.clone(), theme.clone(), settings.units, cx));
        viewport.update(cx, |viewport, cx| {
            viewport.set_nav_preset(settings.nav_preset, cx)
        });
        let saved_revision = document.read(cx).revision;
        let document_subscription = cx.observe(&document, |_app, _, cx| cx.notify());
        let viewport_subscription = cx.subscribe(&viewport, |app, _, event, cx| {
            match event {
                ViewportEvent::ModesExited => {
                    app.active_modes = [false; 4];
                    app.exploded_factor = 0.0;
                    cx.notify();
                }
                ViewportEvent::InteractionChanged => cx.notify(),
            }
        });
        let demo_section = std::env::var("FREE3D_DEMO_SCENE").is_ok_and(|scene| scene == "5");
        let mut recent_files = load_recent_files();
        if let Some(path) = &startup_path {
            recent_files.retain(|recent| recent != path);
            recent_files.insert(0, path.clone());
            recent_files.truncate(8);
            if let Err(error) = save_recent_files(&recent_files) {
                eprintln!("failed to save recent-file list: {error}");
            }
        }
        let home_designs = crate::home::list_designs(&recent_files);
        Self {
            screen: if demo || startup_path.is_some() {
                AppScreen::Workspace
            } else {
                AppScreen::Home
            },
            space: Space::Modeling,
            document,
            viewport,
            project_path: startup_path,
            saved_revision,
            recent_files,
            home_designs,
            home_query: String::new(),
            home_focus: cx.focus_handle(),
            home_menu_path: None,
            home_rename_path: None,
            home_rename_buffer: String::new(),
            home_thumbnails: HashMap::new(),
            theme,
            active_tool: None,
            inspection_card: None,
            prism_sides: 6,
            open_group: None,
            show_items: true,
            show_history: false,
            show_variables: false,
            show_views: false,
            show_command_search: false,
            command_query: String::new(),
            command_highlight: 0,
            command_focus: cx.focus_handle(),
            show_settings: false,
            show_nav_presets: false,
            nav_preset: settings.nav_preset,
            saved_views: SavedViews::default(),
            grid_visible: true,
            grid_before_visualize: None,
            selection_filter_before_visualize: None,
            display_mode: DisplayMode::Shaded,
            analysis: std::env::var("FREE3D_ANALYSIS")
                .is_ok_and(|value| value.eq_ignore_ascii_case("zebra"))
                .then_some(AnalysisMode::Zebra)
                .unwrap_or_default(),
            units: settings.units,
            language: settings.language,
            autosave_interval_secs: settings.autosave_interval_secs,
            snap_enabled: true,
            fov_degrees: 45.0,
            active_modes: [demo_section, false, false, false],
            fov_dragging: false,
            fov_last_x: 0.0,
            _document_subscription: document_subscription,
            _viewport_subscription: viewport_subscription,
            renaming_body: None,
            renaming_plane: None,
            renaming_reference_image: None,
            renaming_variable: None,
            rename_buffer: String::new(),
            rename_focus: cx.focus_handle(),
            last_constraint_conflict: None,
            history_editor: None,
            history_editor_subscription: None,
            variable_editor: None,
            variable_editor_subscription: None,
            material_editor: None,
            material_editor_subscription: None,
            joint_editor: None,
            joint_editor_subscription: None,
            replay_error: None,
            drawing_tool: None,
            drawing_pending_view_at: None,
            drawing_pending_dim: None,
            drawing_pending_section: None,
            drawing_pending_angle: None,
            drawing_title_editor: None,
            drawing_detail_scale: 2.0,
            drawing_numeric_buffer: String::new(),
            drawing_selected_view: None,
            drawing_selected_dim: None,
            drawing_drag: None,
            drawing_cache: HashMap::new(),
            drawing_bom_cache: None,
            exploded_factor: 0.0,
            exploded_dragging: false,
            exploded_last_x: 0.0,
        }
    }

    /// Opens a numeric editor for one history parameter.
    pub(crate) fn begin_history_edit(
        &mut self,
        index: usize,
        field: HistoryNumericField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replay_error = None;
        let value = self.document.update(cx, |document, _| {
            let steps = document.replayable_history();
            steps.get(index).and_then(|step| match field {
                HistoryNumericField::Primary => step.op.numeric_value().map(|value| {
                    let is_length = matches!(
                        step.op,
                        crate::history::HistoryOp::Extrude { .. }
                            | crate::history::HistoryOp::OffsetFace { .. }
                            | crate::history::HistoryOp::Fillet { .. }
                            | crate::history::HistoryOp::Chamfer { .. }
                            | crate::history::HistoryOp::Shell { .. }
                            | crate::history::HistoryOp::Hole { .. }
                            | crate::history::HistoryOp::LinearPattern { .. }
                            | crate::history::HistoryOp::AddReferenceImage { .. }
                    );
                    (value, is_length, step.op.numeric_editor_text())
                }),
                HistoryNumericField::Count => match &step.op {
                    crate::history::HistoryOp::LinearPattern { count, .. } => {
                        Some((f64::from(*count), false, None))
                    }
                    _ => None,
                },
            })
        });
        let Some((value, is_length, editor_text)) = value else {
            cx.notify();
            return;
        };
        let units = self.units;
        let displayed = if is_length {
            units.display_value(value)
        } else {
            value
        };
        let variables = self
            .document
            .read(cx)
            .variables
            .iter()
            .map(|variable| (variable.name.clone(), variable.value))
            .collect();
        let seed = editor_text.unwrap_or_else(|| format!("{displayed:.3}"));
        let input = cx.new(|cx| {
            let input =
                NumericInput::new_with_variables(seed, "", self.theme.clone(), variables, cx);
            if is_length {
                input.with_units(units)
            } else {
                input
            }
        });
        let subscription = cx.subscribe_in(&input, window, |app, _, event, window, cx| {
            app.handle_history_input(event.clone(), window, cx);
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.history_editor = Some((index, field, input));
        self.history_editor_subscription = Some(subscription);
        cx.notify();
    }

    fn handle_history_input(
        &mut self,
        event: NumericInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let NumericInputEvent::Commit { value, expression } = event
            && let Some((index, field, _)) = &self.history_editor
        {
            let (index, field) = (*index, *field);
            let mut steps = self
                .document
                .update(cx, |document, _| document.replayable_history());
            if let Some(step) = steps.get_mut(index) {
                let changed = match field {
                    HistoryNumericField::Primary => step.op.set_numeric_input(value, expression),
                    HistoryNumericField::Count => step.op.set_secondary_count(value),
                };
                if changed {
                    self.commit_history_steps(steps, cx);
                }
            }
        }
        self.history_editor = None;
        self.history_editor_subscription = None;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Adds a variable row and immediately opens its inline name editor.
    pub(crate) fn add_variable_from_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.document.update(cx, |document, cx| {
            let index = document.add_variable();
            cx.notify();
            index
        });
        self.begin_variable_rename(index, window, cx);
    }

    /// Opens the identifier-only inline editor for a variable name.
    pub(crate) fn begin_variable_rename(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .document
            .read(cx)
            .variables
            .get(index)
            .map(|variable| variable.name.clone())
        else {
            return;
        };
        self.renaming_body = None;
        self.renaming_plane = None;
        self.renaming_variable = Some(index);
        self.rename_buffer = name;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Opens a numeric-style expression editor for a variable row.
    pub(crate) fn begin_variable_expression_edit(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let document = self.document.read(cx);
        let Some(variable) = document.variables.get(index) else {
            return;
        };
        let seed = variable.expr.clone();
        let variables = document.variables[..index]
            .iter()
            .map(|variable| (variable.name.clone(), variable.value))
            .collect();
        let input = cx.new(|cx| {
            NumericInput::new_with_variables(seed, "", self.theme.clone(), variables, cx)
        });
        let subscription = cx.subscribe_in(&input, window, |app, _, event, window, cx| {
            app.handle_variable_input(event.clone(), window, cx);
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.variable_editor = Some((index, input));
        self.variable_editor_subscription = Some(subscription);
        cx.notify();
    }

    fn handle_variable_input(
        &mut self,
        event: NumericInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let NumericInputEvent::Commit { expression, .. } = event
            && let Some((index, _)) = &self.variable_editor
        {
            let index = *index;
            self.document.update(cx, |document, cx| {
                if let Some(name) = document
                    .variables
                    .get(index)
                    .map(|variable| variable.name.clone())
                    && document.update_variable(index, name, expression)
                {
                    cx.notify();
                }
            });
        }
        self.variable_editor = None;
        self.variable_editor_subscription = None;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Opens one H/S/L component editor for the selected body's material.
    pub(crate) fn begin_material_edit(
        &mut self,
        body: BodyId,
        field: MaterialNumericField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(material) = self
            .document
            .read(cx)
            .bodies
            .iter()
            .find(|item| item.id == body)
            .map(|item| item.material)
        else {
            return;
        };
        let hsl = rgb_to_hsl(material.base_color);
        let (value, suffix) = match field {
            MaterialNumericField::Hue => (hsl[0], "°"),
            MaterialNumericField::Saturation => (hsl[1], "%"),
            MaterialNumericField::Lightness => (hsl[2], "%"),
        };
        let input =
            cx.new(|cx| NumericInput::new(format!("{value:.1}"), suffix, self.theme.clone(), cx));
        let subscription = cx.subscribe_in(&input, window, |app, _, event, window, cx| {
            app.handle_material_input(event.clone(), window, cx);
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.material_editor = Some((body, field, input));
        self.material_editor_subscription = Some(subscription);
        cx.notify();
    }

    fn handle_material_input(
        &mut self,
        event: NumericInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let NumericInputEvent::Commit { value, .. } = event
            && let Some((body, field, _)) = &self.material_editor
        {
            let body = *body;
            let field = *field;
            self.document.update(cx, |document, cx| {
                if let Some(current) = document.bodies.iter().find(|item| item.id == body) {
                    let mut material = current.material;
                    let mut hsl = rgb_to_hsl(material.base_color);
                    match field {
                        MaterialNumericField::Hue => hsl[0] = value as f32,
                        MaterialNumericField::Saturation => hsl[1] = value as f32,
                        MaterialNumericField::Lightness => hsl[2] = value as f32,
                    }
                    material.base_color = hsl_to_rgb(hsl[0], hsl[1], hsl[2]);
                    document.set_material(body, material);
                    cx.notify();
                }
            });
        }
        self.material_editor = None;
        self.material_editor_subscription = None;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Opens the primary drive-value editor for an assembly joint.
    pub(crate) fn begin_joint_edit(
        &mut self,
        id: JointId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((kind, current)) = self
            .document
            .read(cx)
            .joints
            .iter()
            .find(|joint| joint.id == id)
            .map(|joint| (joint.kind, joint.value))
        else {
            return;
        };
        if matches!(kind, JointKind::Fixed | JointKind::Ball) {
            return;
        }
        let (value, suffix) = if kind == JointKind::Revolute {
            (current.to_degrees(), "°")
        } else {
            (self.units.display_value(current), "")
        };
        let input = cx.new(|cx| {
            let input = NumericInput::new(format!("{value:.3}"), suffix, self.theme.clone(), cx);
            if kind == JointKind::Revolute {
                input
            } else {
                input.with_units(self.units)
            }
        });
        let subscription = cx.subscribe_in(&input, window, |app, _, event, window, cx| {
            app.handle_joint_input(event.clone(), window, cx);
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        self.joint_editor = Some((id, input));
        self.joint_editor_subscription = Some(subscription);
        cx.notify();
    }

    fn handle_joint_input(
        &mut self,
        event: NumericInputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let NumericInputEvent::Commit { value, .. } = event
            && let Some((id, _)) = self.joint_editor
        {
            self.document.update(cx, |document, cx| {
                if let Some(joint) = document.joints.iter().find(|joint| joint.id == id).cloned() {
                    let value = joint_editor_internal_value(joint.kind, self.units, value);
                    document.set_joint_value(id, value, joint.value2);
                    cx.notify();
                }
            });
        }
        self.joint_editor = None;
        self.joint_editor_subscription = None;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Applies a preset material to a body immediately.
    pub(crate) fn apply_material(
        &mut self,
        body: BodyId,
        material: Material,
        cx: &mut Context<Self>,
    ) {
        self.document.update(cx, |document, cx| {
            document.set_material(body, material);
            cx.notify();
        });
        cx.notify();
    }

    /// Toggles suppression and replays the resulting timeline transactionally.
    pub(crate) fn toggle_history_step(&mut self, index: usize, cx: &mut Context<Self>) {
        self.replay_error = None;
        self.history_editor = None;
        self.history_editor_subscription = None;
        let mut steps = self
            .document
            .update(cx, |document, _| document.replayable_history());
        if let Some(step) = steps.get_mut(index) {
            step.suppressed = !step.suppressed;
            self.commit_history_steps(steps, cx);
        }
    }

    /// Deletes a step and replays the resulting timeline transactionally.
    pub(crate) fn delete_history_step(&mut self, index: usize, cx: &mut Context<Self>) {
        self.replay_error = None;
        self.history_editor = None;
        self.history_editor_subscription = None;
        let mut steps = self
            .document
            .update(cx, |document, _| document.replayable_history());
        if index < steps.len() {
            steps.remove(index);
            self.commit_history_steps(steps, cx);
        }
    }

    fn commit_history_steps(
        &mut self,
        steps: Vec<crate::history::HistoryStep>,
        cx: &mut Context<Self>,
    ) {
        let result = self
            .document
            .update(cx, |document, _| document.replace_history(steps));
        match result {
            Ok(()) => self.replay_error = None,
            Err(error) => {
                eprintln!(
                    "history replay failed at step {}: {}",
                    error.step_index, error.message
                );
                self.replay_error = Some((error.step_index, error.message));
            }
        }
        cx.notify();
    }

    fn selected_sketch_entities(
        &self,
        cx: &Context<Self>,
    ) -> Option<(crate::sketch::SketchId, Vec<usize>)> {
        let document = self.document.read(cx);
        let id = document.active_sketch?;
        let entities: Vec<_> = document
            .selection
            .items
            .iter()
            .filter_map(|item| match item {
                SelItem::SketchEntity(selected_id, entity) if *selected_id == id => Some(*entity),
                _ => None,
            })
            .collect();
        (!entities.is_empty()).then_some((id, entities))
    }

    /// Whether the current curve selection supports a panel constraint action.
    pub fn constraint_enabled(&self, kind: SketchConstraintKind, cx: &Context<Self>) -> bool {
        let Some((id, selected)) = self.selected_sketch_entities(cx) else {
            return false;
        };
        let document = self.document.read(cx);
        let Some(sketch) = document.sketches.iter().find(|sketch| sketch.id == id) else {
            return false;
        };
        let is_line = |index: usize| {
            matches!(
                sketch.entities.get(index).map(|item| &item.geo),
                Some(SketchEntity::Line { .. })
            )
        };
        let is_circle = |index: usize| {
            matches!(
                sketch.entities.get(index).map(|item| &item.geo),
                Some(SketchEntity::Circle { .. })
            )
        };
        let has_coincident_point = |index: usize| {
            matches!(
                sketch.entities.get(index).map(|item| &item.geo),
                Some(
                    SketchEntity::Line { .. }
                        | SketchEntity::Arc { .. }
                        | SketchEntity::Spline { .. }
                        | SketchEntity::CvSpline { .. }
                        | SketchEntity::EllipseArc { .. }
                        | SketchEntity::Point { .. }
                )
            )
        };
        let is_point_target = |index: usize| {
            matches!(
                sketch.entities.get(index).map(|item| &item.geo),
                Some(
                    SketchEntity::Line { .. }
                        | SketchEntity::Circle { .. }
                        | SketchEntity::Arc { .. }
                )
            )
        };
        match kind {
            SketchConstraintKind::Construction => true,
            SketchConstraintKind::Horizontal | SketchConstraintKind::Vertical => {
                selected.iter().all(|&index| is_line(index))
            }
            SketchConstraintKind::Parallel
            | SketchConstraintKind::Perpendicular
            | SketchConstraintKind::Collinear => {
                selected.len() == 2 && selected.iter().all(|&index| is_line(index))
            }
            SketchConstraintKind::Fix => true,
            SketchConstraintKind::G2 => {
                selected.len() == 2
                    && selected.iter().any(|&index| {
                        matches!(
                            sketch.entities[index].geo,
                            SketchEntity::Spline { .. } | SketchEntity::CvSpline { .. }
                        )
                    })
            }
            SketchConstraintKind::Coincident => {
                selected.len() == 2 && selected.iter().all(|&index| has_coincident_point(index))
            }
            SketchConstraintKind::Equal => {
                selected.len() == 2
                    && (selected.iter().all(|&index| is_line(index))
                        || selected.iter().all(|&index| is_circle(index)))
            }
            SketchConstraintKind::Tangent => {
                selected.len() == 2
                    && selected.iter().filter(|&&index| is_line(index)).count() == 1
                    && selected.iter().filter(|&&index| is_circle(index)).count() == 1
            }
            SketchConstraintKind::Concentric => {
                selected.len() == 2 && selected.iter().all(|&index| is_circle(index))
            }
            SketchConstraintKind::Symmetric => {
                selected.len() == 3
                    && has_coincident_point(selected[0])
                    && has_coincident_point(selected[1])
                    && is_line(selected[2])
            }
            SketchConstraintKind::PointOnObject => {
                selected.len() == 2
                    && has_coincident_point(selected[0])
                    && is_point_target(selected[1])
            }
        }
    }

    /// Builds and transactionally applies the requested constraint.
    pub fn apply_sketch_constraint(&mut self, kind: SketchConstraintKind, cx: &mut Context<Self>) {
        self.last_constraint_conflict = None;
        if !self.constraint_enabled(kind, cx) {
            cx.notify();
            return;
        }
        let Some((id, selected)) = self.selected_sketch_entities(cx) else {
            return;
        };
        if kind == SketchConstraintKind::Construction {
            self.document.update(cx, |document, cx| {
                document.toggle_sketch_construction(id, &selected);
                cx.notify();
            });
            cx.notify();
            return;
        }
        if kind == SketchConstraintKind::Fix {
            self.document.update(cx, |document, cx| {
                document.toggle_sketch_fix(id, &selected);
                cx.notify();
            });
            return;
        }
        let sketch = self
            .document
            .read(cx)
            .sketches
            .iter()
            .find(|sketch| sketch.id == id)
            .cloned()
            .expect("enabled constraint has an active sketch");
        let reference = |index| EntityRef(selected[index]);
        let points = |entity: usize| match &sketch.entities[entity].geo {
            SketchEntity::Line { a, b } => vec![*a, *b],
            SketchEntity::Arc { start, end, .. } => vec![*start, *end],
            SketchEntity::Spline { points } if points.len() >= 2 => {
                vec![points[0], *points.last().expect("spline endpoint")]
            }
            SketchEntity::CvSpline { control, .. } if control.len() >= 2 => {
                vec![control[0], *control.last().expect("CV spline endpoint")]
            }
            SketchEntity::EllipseArc {
                center,
                major,
                minor_ratio,
                start_angle,
                end_angle,
            } => vec![
                ellipse_point(*center, *major, *minor_ratio, *start_angle),
                ellipse_point(*center, *major, *minor_ratio, *end_angle),
            ],
            SketchEntity::Point { at } => vec![*at],
            _ => Vec::new(),
        };
        let mut constraints = match kind {
            SketchConstraintKind::Construction => unreachable!("handled above"),
            SketchConstraintKind::Fix => unreachable!("handled above"),
            SketchConstraintKind::Horizontal => selected
                .iter()
                .map(|&index| Constraint::Horizontal(EntityRef(index)))
                .collect(),
            SketchConstraintKind::Vertical => selected
                .iter()
                .map(|&index| Constraint::Vertical(EntityRef(index)))
                .collect(),
            SketchConstraintKind::Parallel => {
                vec![Constraint::Parallel(reference(0), reference(1))]
            }
            SketchConstraintKind::Perpendicular => {
                vec![Constraint::Perpendicular(reference(0), reference(1))]
            }
            SketchConstraintKind::Collinear => {
                vec![Constraint::Collinear(reference(0), reference(1))]
            }
            SketchConstraintKind::G2 => {
                let spline_position = selected
                    .iter()
                    .position(|&index| {
                        matches!(
                            sketch.entities[index].geo,
                            SketchEntity::Spline { .. } | SketchEntity::CvSpline { .. }
                        )
                    })
                    .expect("enabled G2 has spline");
                let curve_position = 1 - spline_position;
                let spline_points = points(selected[spline_position]);
                let curve_points = points(selected[curve_position]);
                let (spline_end, curve_end, _) = (0..2)
                    .flat_map(|a| (0..2).map(move |b| (a, b)))
                    .map(|(a, b)| (a, b, spline_points[a].distance_squared(curve_points[b])))
                    .min_by(|a, b| a.2.total_cmp(&b.2))
                    .expect("G2 endpoints");
                vec![Constraint::G2 {
                    spline: EntityRef(selected[spline_position]),
                    curve: EntityRef(selected[curve_position]),
                    spline_end: spline_end as u8,
                    curve_end: curve_end as u8,
                }]
            }
            SketchConstraintKind::Equal => vec![Constraint::Equal(reference(0), reference(1))],
            SketchConstraintKind::Concentric => {
                vec![Constraint::Concentric(reference(0), reference(1))]
            }
            SketchConstraintKind::Tangent => {
                let line = selected
                    .iter()
                    .copied()
                    .find(|&index| matches!(sketch.entities[index].geo, SketchEntity::Line { .. }))
                    .expect("enabled tangent has a line");
                let circle = selected
                    .iter()
                    .copied()
                    .find(|&index| {
                        matches!(sketch.entities[index].geo, SketchEntity::Circle { .. })
                    })
                    .expect("enabled tangent has a circle");
                vec![Constraint::Tangent {
                    line: EntityRef(line),
                    circle: EntityRef(circle),
                }]
            }
            SketchConstraintKind::Coincident => {
                let a = points(selected[0]);
                let b = points(selected[1]);
                let (a_point, b_point, _) = (0..a.len())
                    .flat_map(|a_point| (0..b.len()).map(move |b_point| (a_point, b_point)))
                    .map(|(a_point, b_point)| {
                        (a_point, b_point, a[a_point].distance_squared(b[b_point]))
                    })
                    .min_by(|left, right| left.2.total_cmp(&right.2))
                    .expect("two lines have endpoints");
                vec![Constraint::Coincident {
                    a: PointRef {
                        entity: selected[0],
                        point: a_point as u8,
                    },
                    b: PointRef {
                        entity: selected[1],
                        point: b_point as u8,
                    },
                }]
            }
            SketchConstraintKind::Symmetric => {
                let a = points(selected[0]);
                let b = points(selected[1]);
                let (a_point, b_point, _) = (0..a.len())
                    .flat_map(|a_point| (0..b.len()).map(move |b_point| (a_point, b_point)))
                    .map(|(a_point, b_point)| {
                        (a_point, b_point, a[a_point].distance_squared(b[b_point]))
                    })
                    .min_by(|left, right| left.2.total_cmp(&right.2))
                    .expect("enabled symmetry has endpoints");
                vec![Constraint::Symmetric {
                    a: PointRef {
                        entity: selected[0],
                        point: a_point as u8,
                    },
                    b: PointRef {
                        entity: selected[1],
                        point: b_point as u8,
                    },
                    axis: EntityRef(selected[2]),
                }]
            }
            SketchConstraintKind::PointOnObject => {
                let candidates = points(selected[0]);
                let target = &sketch.entities[selected[1]].geo;
                let distance = |point: glam::DVec2| match target {
                    SketchEntity::Line { a, b } => {
                        (b - a).perp_dot(point - *a).abs() / b.distance(*a).max(1.0e-12)
                    }
                    SketchEntity::Circle { center, radius } => {
                        (point.distance(*center) - radius).abs()
                    }
                    SketchEntity::Arc { start, end, mid } => arc_center_radius(*start, *mid, *end)
                        .map(|(center, radius)| (point.distance(center) - radius).abs())
                        .unwrap_or(f64::INFINITY),
                    _ => f64::INFINITY,
                };
                let point = candidates
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| distance(**a).total_cmp(&distance(**b)))
                    .map(|(index, _)| index)
                    .expect("enabled point-on-object has points");
                vec![Constraint::PointOnObject {
                    point: PointRef {
                        entity: selected[0],
                        point: point as u8,
                    },
                    target: EntityRef(selected[1]),
                }]
            }
        };
        let mut committed = false;
        self.document.update(cx, |document, cx| {
            committed = if constraints.len() == 1 {
                document.add_constraint(id, constraints.remove(0))
            } else {
                document.add_constraints(id, constraints)
            };
            if committed {
                cx.notify();
            }
        });
        if !committed {
            eprintln!("constraint solve did not converge; {kind:?} rejected");
            self.last_constraint_conflict = Some(kind);
        }
        cx.notify();
    }

    /// Whether the given mode chip is currently toggled on.
    pub fn mode_active(&self, chip: ModeChip) -> bool {
        self.active_modes[Self::mode_index(chip)]
    }

    /// Whether the mode currently has enough context to be activated.
    pub fn mode_enabled(&self, chip: ModeChip, cx: &Context<Self>) -> bool {
        chip != ModeChip::Isolate
            || self.mode_active(chip)
            || self
                .document
                .read(cx)
                .selection
                .items
                .iter()
                .any(|item| matches!(item, SelItem::Body(_)))
    }

    fn mode_index(chip: ModeChip) -> usize {
        match chip {
            ModeChip::Section => 0,
            ModeChip::Isolate => 1,
            ModeChip::Measure => 2,
            ModeChip::Exploded => 3,
        }
    }

    /// Interprets a chrome command, updating state and the viewport as needed.
    pub fn dispatch(&mut self, command: AppCommand, window: &mut Window, cx: &mut Context<Self>) {
        match command {
            AppCommand::Undo => {
                self.document.update(cx, |document, cx| {
                    if self.space == Space::Drawing {
                        if document.drawing.undo() {
                            document.drawing_changed();
                        }
                    } else {
                        document.undo();
                    }
                    cx.notify();
                });
                if self.space == Space::Drawing {
                    self.drawing_cache.clear();
                    self.drawing_selected_view = None;
                }
            }
            AppCommand::Redo => {
                self.document.update(cx, |document, cx| {
                    if self.space == Space::Drawing {
                        if document.drawing.redo() {
                            document.drawing_changed();
                        }
                    } else {
                        document.redo();
                    }
                    cx.notify();
                });
                if self.space == Space::Drawing {
                    self.drawing_cache.clear();
                    self.drawing_selected_view = None;
                }
            }
            AppCommand::SaveProject => self.save_project(cx),
            AppCommand::SaveProjectAs => self.save_project_as(cx),
            AppCommand::OpenProject => self.open_project_with_confirmation(window, cx),
            AppCommand::NewProject => self.new_project_with_confirmation(window, cx),
            AppCommand::Import => self.import(cx),
            AppCommand::Export => self.export(cx),
            AppCommand::CommandSearch => {
                self.show_command_search = true;
                self.command_query.clear();
                self.command_highlight = 0;
                window.focus(&self.command_focus, cx);
            }
            AppCommand::VisualizeSpace => self.set_space(Space::Visualize, cx),
            AppCommand::OpenSettings => {
                self.show_settings = !self.show_settings;
                self.show_nav_presets = false;
            }
            AppCommand::ActivateTool(tool) => {
                self.active_tool = Some(tool);
                self.open_group = None;
                self.viewport.update(cx, |viewport, _| {
                    viewport.set_joint_drive_enabled(tool == ToolId::Drive)
                });
                if tool == ToolId::Measure {
                    let index = Self::mode_index(ModeChip::Measure);
                    self.active_modes[index] = true;
                    self.viewport.update(cx, |viewport, cx| {
                        viewport.set_mode(ModeChip::Measure, true, window, cx)
                    });
                }
                if tool == ToolId::Ground {
                    self.document.update(cx, |document, cx| {
                        let bodies: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| item.body_id())
                            .collect();
                        for body in bodies {
                            document.toggle_grounded(body);
                        }
                        cx.notify();
                    });
                }
                if tool == ToolId::Joint {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.begin_joint_tool(cx));
                }
                if matches!(
                    tool,
                    ToolId::Properties | ToolId::InterferenceCheck | ToolId::GeometryCheck
                ) {
                    self.open_inspection(tool, cx);
                }
                if matches!(
                    tool,
                    ToolId::Line
                        | ToolId::Rectangle
                        | ToolId::CenterRectangle
                        | ToolId::RoundedRectangle
                        | ToolId::Polygon
                        | ToolId::Slot
                        | ToolId::Circle
                        | ToolId::ThreePointCircle
                        | ToolId::Ellipse
                        | ToolId::EllipseArc
                        | ToolId::Arc
                        | ToolId::Point
                        | ToolId::TangentArc
                        | ToolId::Spline
                        | ToolId::CvSpline
                        | ToolId::TwoTangentCircle
                        | ToolId::ThreeTangentCircle
                        | ToolId::SketchFillet
                        | ToolId::Trim
                        | ToolId::Extend
                        | ToolId::Break
                        | ToolId::SketchOffset
                ) {
                    let (id, plane) = self.document.update(cx, |document, cx| {
                        if let Some(id) = document.active_sketch
                            && let Some(sketch) =
                                document.sketches.iter().find(|sketch| sketch.id == id)
                        {
                            return (id, sketch.plane);
                        }
                        let selected_plane = document
                            .selection
                            .items
                            .iter()
                            .find_map(|item| match *item {
                                SelItem::Face(body_id, face_index) => document
                                    .bodies
                                    .iter()
                                    .find(|body| body.id == body_id)
                                    .and_then(|body| {
                                        SketchPlane::from_face(&body.shape, face_index)
                                            .map(|plane| (plane, Some(body_id)))
                                    }),
                                SelItem::Plane(id) => document
                                    .construction_planes
                                    .iter()
                                    .find(|plane| plane.id == id)
                                    .map(|plane| (plane.plane, None)),
                                _ => None,
                            })
                            .unwrap_or_else(|| (SketchPlane::xy(), None));
                        let (plane, support_body) = selected_plane;
                        let id = document.add_sketch_with_support(plane, support_body);
                        document.selection.clear();
                        cx.notify();
                        (id, plane)
                    });
                    self.viewport.update(cx, |viewport, cx| {
                        viewport.enter_sketch(id, plane, tool, window, cx)
                    });
                }
                let boolean = match tool {
                    ToolId::Union => Some(BooleanOp::Union),
                    ToolId::Subtract => Some(BooleanOp::Subtract),
                    ToolId::Intersect => Some(BooleanOp::Intersect),
                    _ => None,
                };
                if let Some(operation) = boolean {
                    let applied = self.document.update(cx, |document, cx| {
                        let ids: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                SelItem::Body(id) => Some(*id),
                                _ => None,
                            })
                            .collect();
                        if document.apply_boolean(operation, &ids) {
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    });
                    if !applied {
                        self.viewport.update(cx, |viewport, cx| {
                            viewport.show_modeling_hint(
                                crate::i18n::t("Select at least two bodies; boolean operations do not support surface bodies"),
                                window,
                                cx,
                            )
                        });
                    }
                }
                if tool == ToolId::Thread {
                    let selection = self.document.read(cx);
                    let selected = (|| {
                        let document = &*selection;
                        let [SelItem::Face(body, face)] = document.selection.items.as_slice()
                        else {
                            return None;
                        };
                        let (body, face) = (*body, *face);
                        let depth = document
                            .bodies
                            .iter()
                            .find(|candidate| candidate.id == body)
                            .and_then(|body| body.shape.face_cylinder_data(face as usize).ok())
                            .map(|(_, _, _, height)| height)?;
                        Some((body, face, depth))
                    })();
                    if let Some((body, face, depth)) = selected {
                        self.viewport.update(cx, |viewport, cx| {
                            viewport.activate_thread(body, face, depth, window, cx)
                        });
                    } else {
                        self.viewport.update(cx, |viewport, cx| {
                            viewport.show_modeling_hint(
                                crate::i18n::t("Select a cylindrical face"),
                                window,
                                cx,
                            )
                        });
                    }
                }
                if tool == ToolId::Patch {
                    let applied = self.document.update(cx, |document, cx| {
                        let selected: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                SelItem::Edge(body, edge) => Some((*body, *edge)),
                                _ => None,
                            })
                            .collect();
                        if let Some((body, _)) = selected.first()
                            && selected.iter().all(|(candidate, _)| candidate == body)
                            && document
                                .apply_patch(
                                    *body,
                                    &selected.iter().map(|(_, edge)| *edge).collect::<Vec<_>>(),
                                )
                                .is_some()
                        {
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    });
                    if !applied {
                        self.viewport.update(cx, |viewport, cx| {
                            viewport.show_modeling_hint(
                                crate::i18n::t(
                                    "Select one complete closed boundary on the same body",
                                ),
                                window,
                                cx,
                            )
                        });
                    }
                }
                if tool == ToolId::Stitch {
                    let applied = self.document.update(cx, |document, cx| {
                        let ids: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                SelItem::Body(id) => Some(*id),
                                _ => None,
                            })
                            .collect();
                        if document.apply_stitch(&ids).is_some() {
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    });
                    if !applied {
                        self.viewport.update(cx, |viewport, cx| {
                            viewport.show_modeling_hint(
                                crate::i18n::t("Select at least two surface bodies to stitch"),
                                window,
                                cx,
                            )
                        });
                    }
                }
                if tool == ToolId::DeleteFace {
                    let applied = self.document.update(cx, |document, cx| {
                        let selected: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                SelItem::Face(body, face) => Some((*body, *face)),
                                _ => None,
                            })
                            .collect();
                        if let Some((body, _)) = selected.first()
                            && selected.iter().all(|(candidate, _)| candidate == body)
                            && document.apply_delete_faces(
                                *body,
                                &selected.iter().map(|(_, face)| *face).collect::<Vec<_>>(),
                            )
                        {
                            cx.notify();
                            true
                        } else {
                            false
                        }
                    });
                    if !applied {
                        self.viewport.update(cx, |viewport, cx| {
                            viewport.show_modeling_hint(
                                crate::i18n::t("The deleted face could not heal into a valid body; the model was not changed"),
                                window,
                                cx,
                            )
                        });
                    }
                }
                if tool == ToolId::Mirror {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_mirror(window, cx));
                }
                if tool == ToolId::Revolve {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_revolve(window, cx));
                }
                if tool == ToolId::ReplaceFace {
                    self.viewport.update(cx, |viewport, cx| {
                        viewport.activate_replace_face(window, cx)
                    });
                }
                if tool == ToolId::Project {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_project(window, cx));
                }
                if tool == ToolId::Helix {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_helix(window, cx));
                }
                if tool == ToolId::Hole {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_hole(window, cx));
                }
                if tool == ToolId::Draft {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_draft(window, cx));
                }
                if tool == ToolId::Sweep {
                    self.document.update(cx, |document, cx| {
                        let profiles: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                SelItem::Profile(sketch, profile) => Some((*sketch, *profile)),
                                _ => None,
                            })
                            .collect();
                        let path = if profiles.len() >= 2 {
                            Some(PathRef::Profile {
                                sketch: profiles[1].0,
                                profile_index: profiles[1].1,
                            })
                        } else if profiles.len() == 1 {
                            let entities: Vec<_> = document
                                .selection
                                .items
                                .iter()
                                .filter_map(|item| match item {
                                    SelItem::SketchEntity(sketch, entity) => {
                                        Some((*sketch, *entity))
                                    }
                                    _ => None,
                                })
                                .collect();
                            entities.first().and_then(|(sketch, _)| {
                                (document.selection.items.len() == entities.len() + 1
                                    && entities.iter().all(|(id, _)| id == sketch))
                                .then(|| PathRef::OpenChain {
                                    sketch: *sketch,
                                    entity_indices: entities
                                        .iter()
                                        .map(|(_, entity)| *entity)
                                        .collect(),
                                })
                            })
                        } else {
                            None
                        };
                        if let Some(path) = path
                            && document.apply_sweep(profiles[0], path).is_some()
                        {
                            cx.notify();
                        }
                    });
                }
                if tool == ToolId::Loft {
                    self.document.update(cx, |document, cx| {
                        let sections: Vec<_> = document
                            .selection
                            .items
                            .iter()
                            .filter_map(|item| match item {
                                SelItem::Profile(sketch, profile) => Some((*sketch, *profile)),
                                _ => None,
                            })
                            .collect();
                        if sections.len() >= 2 && document.apply_loft(&sections).is_some() {
                            cx.notify();
                        }
                    });
                }
                if tool == ToolId::Pattern {
                    self.viewport
                        .update(cx, |viewport, cx| viewport.activate_pattern(window, cx));
                }
                if matches!(
                    tool,
                    ToolId::Scale | ToolId::Split | ToolId::Align | ToolId::Plane
                ) {
                    self.viewport.update(cx, |viewport, cx| {
                        viewport.activate_m6_tool(tool, window, cx)
                    });
                }
                if tool == ToolId::Axis {
                    self.document.update(cx, |document, cx| {
                        let picked = document
                            .selection
                            .items
                            .iter()
                            .find_map(|item| match *item {
                                SelItem::Edge(body, edge) => {
                                    let body =
                                        document.bodies.iter().find(|item| item.id == body)?;
                                    let a = body.shape.edge_start_point(edge as usize).ok()?;
                                    let b = body.shape.edge_end_point(edge as usize).ok()?;
                                    Some((a, b - a))
                                }
                                _ => None,
                            });
                        let (origin, direction) = picked.unwrap_or((DVec3::ZERO, DVec3::Z));
                        if document.add_construction_axis(origin, direction).is_some() {
                            cx.notify();
                        }
                    });
                }
                if tool == ToolId::DatumPoint {
                    self.document.update(cx, |document, cx| {
                        if document.add_construction_point(DVec3::ZERO).is_some() {
                            cx.notify();
                        }
                    });
                }
                if tool == ToolId::ReferenceImage {
                    self.insert_reference_image(cx);
                }
                if matches!(
                    tool,
                    ToolId::Fillet
                        | ToolId::Chamfer
                        | ToolId::Shell
                        | ToolId::Thicken
                        | ToolId::OffsetFace
                ) {
                    self.viewport.update(cx, |viewport, cx| {
                        viewport.activate_drag_tool(tool, window, cx)
                    });
                }
                if let Some(kind) = self.primitive(tool) {
                    self.document.update(cx, |document, cx| {
                        let id = document.add_primitive(kind);
                        document.selection.items = vec![SelItem::Body(id)];
                        cx.notify();
                    });
                }
            }
            AppCommand::StandardView(view) => {
                self.show_views = false;
                self.viewport.update(cx, |viewport, cx| {
                    viewport.go_to_standard_view(view, window, cx)
                });
            }
            AppCommand::ToggleItemsPanel => self.show_items = !self.show_items,
            AppCommand::ToggleHistoryPanel => self.show_history = !self.show_history,
            AppCommand::ToggleVariablesPanel => self.show_variables = !self.show_variables,
            AppCommand::ToggleMode(chip) => {
                if chip == ModeChip::Exploded {
                    let factor = if self.exploded_factor > 0.0 { 0.0 } else { 1.0 };
                    self.set_exploded_factor(factor, window, cx);
                    return;
                }
                let index = Self::mode_index(chip);
                if self.mode_enabled(chip, cx) {
                    self.active_modes[index] = !self.active_modes[index];
                    let active = self.active_modes[index];
                    self.viewport.update(cx, |viewport, cx| {
                        viewport.set_mode(chip, active, window, cx)
                    });
                }
            }
            AppCommand::ToggleWireframe => {
                self.display_mode = self.display_mode.next();
                let display_mode = self.display_mode;
                self.viewport.update(cx, |viewport, cx| {
                    viewport.set_display_mode(display_mode, window, cx)
                });
            }
            AppCommand::ToggleGrid => {
                self.grid_visible = !self.grid_visible;
                let visible = self.grid_visible;
                self.viewport.update(cx, |viewport, cx| {
                    viewport.set_grid_visible(visible, window, cx)
                });
            }
            AppCommand::ToggleSnap => {
                self.snap_enabled = !self.snap_enabled;
                let enabled = self.snap_enabled;
                self.viewport
                    .update(cx, |viewport, _| viewport.set_snap_enabled(enabled));
            }
            AppCommand::Screenshot => self.screenshot(cx),
        }
        cx.notify();
    }

    fn open_inspection(&mut self, tool: ToolId, cx: &mut Context<Self>) {
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
        let bodies: Vec<_> = document
            .bodies
            .iter()
            .filter(|body| ids.contains(&body.id))
            .collect();
        self.inspection_card = Some(match tool {
            ToolId::Properties => InspectionCard::Properties(aggregate_properties(bodies)),
            ToolId::InterferenceCheck => {
                InspectionCard::Interference(find_interferences(bodies, usize::MAX))
            }
            ToolId::GeometryCheck => {
                let Some(body) = bodies.first() else {
                    self.inspection_card = Some(InspectionCard::Validity {
                        body_name: String::new(),
                        issues: Err(crate::i18n::t("No body selected").to_owned()),
                    });
                    return;
                };
                InspectionCard::Validity {
                    body_name: body.name.clone(),
                    issues: body.shape.check().map_err(|error| error.to_string()),
                }
            }
            ToolId::Measure => return,
            _ => return,
        });
        self.viewport.update(cx, |viewport, cx| {
            viewport.show_interference_shape(None, cx)
        });
        cx.notify();
    }

    /// Selects and highlights an interference result pair.
    pub(crate) fn select_interference(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(InspectionCard::Interference(Ok(pairs))) = &self.inspection_card else {
            return;
        };
        let Some(pair) = pairs.get(index) else {
            return;
        };
        let (first, second, shape) = (pair.first, pair.second, Arc::clone(&pair.shape));
        self.document.update(cx, |document, cx| {
            document.selection.items = vec![SelItem::Body(first), SelItem::Body(second)];
            cx.notify();
        });
        self.viewport.update(cx, |viewport, cx| {
            viewport.show_interference_shape(Some(&shape), cx)
        });
        cx.notify();
    }

    /// Dismisses the inspection card and its view-only overlay.
    pub(crate) fn close_inspection(&mut self, cx: &mut Context<Self>) {
        self.inspection_card = None;
        self.viewport.update(cx, |viewport, cx| {
            viewport.show_interference_shape(None, cx)
        });
        cx.notify();
    }

    /// Encodes the latest viewport frame to a numbered screenshot PNG on the Desktop.
    fn screenshot(&mut self, cx: &mut Context<Self>) {
        let Some((width, height, mut bytes)) = self.viewport.read(cx).latest_frame() else {
            return;
        };
        // The frame is BGRA; swap to RGBA for the PNG encoder.
        for pixel in bytes.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let Some(image) = image::RgbaImage::from_raw(width, height, bytes) else {
            eprintln!("screenshot: renderer returned an invalid frame size");
            return;
        };
        let Some(desktop) = dirs::desktop_dir().or_else(dirs::home_dir) else {
            eprintln!("screenshot: no desktop or home directory");
            return;
        };
        let path = (1..)
            .map(|n| desktop.join(format!("Free3D-{}-{n}.png", crate::i18n::t("Screenshot"))))
            .find(|candidate| !candidate.exists())
            .expect("an unused screenshot filename exists");
        if let Err(error) = image.save(&path) {
            eprintln!("screenshot: failed to save {}: {error}", path.display());
        }
    }

    fn insert_reference_image(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::t("Reference image (.png .jpg .jpeg)").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else {
                return;
            };
            let Some(path) = paths.into_iter().next().filter(|path| {
                matches!(
                    path.extension()
                        .and_then(|extension| extension.to_str())
                        .map(str::to_ascii_lowercase)
                        .as_deref(),
                    Some("png" | "jpg" | "jpeg")
                )
            }) else {
                return;
            };
            let loaded = cx
                .background_spawn({
                    let path = path.clone();
                    async move { std::fs::read(path) }
                })
                .await;
            let Ok(bytes) = loaded else {
                return;
            };
            let name = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("Reference Image")
                .to_owned();
            this.update(cx, |this, cx| {
                this.document.update(cx, |document, cx| {
                    document.add_reference_image(name, bytes, 100.0);
                    cx.notify();
                });
                this.active_tool = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(crate::i18n::t("Import STEP / IGES / STL / OBJ / DXF").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else {
                return;
            };
            let paths: Vec<_> = paths
                .into_iter()
                .filter(|path| {
                    matches!(
                        path.extension()
                            .and_then(|extension| extension.to_str())
                            .map(str::to_ascii_lowercase)
                            .as_deref(),
                        Some("step" | "stp" | "stl" | "obj" | "iges" | "igs" | "dxf")
                    )
                })
                .collect();
            let loaded = cx
                .background_spawn(async move {
                    paths
                        .into_iter()
                        .map(|path| {
                            let stem = path
                                .file_stem()
                                .and_then(|stem| stem.to_str())
                                .filter(|stem| !stem.is_empty())
                                .unwrap_or("Imported")
                                .to_owned();
                            if path
                                .extension()
                                .and_then(|extension| extension.to_str())
                                .is_some_and(|extension| extension.eq_ignore_ascii_case("dxf"))
                            {
                                return Ok((path, stem, Vec::new()));
                            }
                            let loaded = match path
                                .extension()
                                .and_then(|extension| extension.to_str())
                                .map(str::to_ascii_lowercase)
                                .as_deref()
                            {
                                Some("stl") => {
                                    Shape::read_stl(&path).map_err(|error| error.to_string())
                                }
                                Some("obj") => crate::io_formats::read_obj_shape(&path),
                                Some("iges" | "igs") => {
                                    Shape::read_iges(&path).map_err(|error| error.to_string())
                                }
                                _ => Shape::read_step(&path).map_err(|error| error.to_string()),
                            };
                            loaded
                                .and_then(|shape| {
                                    shape.to_brep_data().map_err(|error| error.to_string())
                                })
                                .map(|bytes| (path.clone(), stem, bytes))
                                .map_err(|error| {
                                    format!("failed to read {}: {error}", path.display())
                                })
                        })
                        .collect::<Result<Vec<_>, _>>()
                })
                .await;
            match loaded {
                Ok(shapes) => {
                    this.update(cx, |this, cx| {
                        this.document.update(cx, |document, cx| {
                            for (path, stem, bytes) in shapes {
                                if bytes.is_empty() {
                                    if let Err(error) = document.import_file(&path) {
                                        eprintln!("failed to import DXF: {error}");
                                    }
                                    continue;
                                }
                                let shape = match Shape::from_brep_data(&bytes) {
                                    Ok(shape) => shape,
                                    Err(error) => {
                                        eprintln!("failed to transfer imported geometry: {error}");
                                        continue;
                                    }
                                };
                                if let Err(error) = document.add_imported_step(path, stem, shape) {
                                    eprintln!("failed to add imported geometry: {error}");
                                }
                            }
                            cx.notify();
                        });
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => eprintln!("{error}"),
            }
        })
        .detach();
    }

    fn export(&mut self, cx: &mut Context<Self>) {
        if self.space == Space::Drawing {
            self.refresh_drawing_cache(cx);
            let drawing = self.document.read(cx).drawing.clone();
            let projections = self.drawing_projections(cx);
            let bom_rows = self.drawing_bom_rows().to_vec();
            let prompt = cx.prompt_for_new_path(std::path::Path::new(""), Some("Untitled.svg"));
            cx.spawn(async move |_this, _cx| {
                let Ok(Ok(Some(path))) = prompt.await else {
                    return;
                };
                let result = match path.extension().and_then(|extension| extension.to_str()) {
                    Some(extension) if extension.eq_ignore_ascii_case("pdf") => {
                        crate::drawing::export_pdf(&path, &drawing, &projections, &bom_rows)
                    }
                    Some(extension) if extension.eq_ignore_ascii_case("svg") => {
                        crate::drawing::export_svg(&path, &drawing, &projections, &bom_rows)
                    }
                    _ => {
                        Err(crate::i18n::t("Drawing export supports only .svg or .pdf").to_owned())
                    }
                };
                if let Err(error) = result {
                    eprintln!("{error}");
                }
            })
            .detach();
            return;
        }
        let empty = self.document.update(cx, |document, cx| {
            document.replayable_history();
            cx.notify();
            document.bodies.is_empty() && document.sketches.is_empty()
        });
        if empty {
            return;
        }
        let prompt = cx.prompt_for_new_path(std::path::Path::new(""), Some("Untitled.step"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = prompt.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.document.update(cx, |document, _| {
                    if let Err(error) = document.export(&path) {
                        eprintln!("{error}");
                    }
                });
            })
            .ok();
        })
        .detach();
    }

    /// Returns whether the document differs from the last saved or loaded state.
    pub fn is_dirty(&self, cx: &Context<Self>) -> bool {
        self.document.read(cx).revision != self.saved_revision
    }

    /// Project stem plus the unsaved marker used by both title bars.
    pub fn project_label(&self, cx: &Context<Self>) -> String {
        let name = self
            .project_path
            .as_deref()
            .and_then(std::path::Path::file_stem)
            .and_then(std::ffi::OsStr::to_str)
            .filter(|name| !name.is_empty())
            .unwrap_or(crate::i18n::t("Untitled"));
        if self.is_dirty(cx) {
            format!("{name} •")
        } else {
            name.to_owned()
        }
    }

    fn save_project(&mut self, cx: &mut Context<Self>) {
        if let Some(path) = self.project_path.clone() {
            self.save_project_to(path, cx);
        } else {
            self.save_project_as(cx);
        }
    }

    fn save_project_as(&mut self, cx: &mut Context<Self>) {
        let suggested = self
            .project_path
            .as_deref()
            .and_then(std::path::Path::file_name)
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or(crate::i18n::t("Untitled.f3d"));
        let designs_dir = crate::home::designs_dir();
        if let Err(error) = std::fs::create_dir_all(&designs_dir) {
            eprintln!("failed to create designs folder: {error}");
            return;
        }
        let prompt = cx.prompt_for_new_path(&designs_dir, Some(suggested));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut path))) = prompt.await else {
                return;
            };
            if !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("f3d"))
            {
                path.set_extension("f3d");
            }
            this.update(cx, |this, cx| this.save_project_to(path, cx))
                .ok();
        })
        .detach();
    }

    fn save_project_to(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if path.parent() == Some(crate::home::designs_dir().as_path())
            && let Err(error) = std::fs::create_dir_all(crate::home::designs_dir())
        {
            eprintln!("failed to create designs folder: {error}");
            return;
        }
        self.capture_thumbnail(cx);
        let result = self
            .document
            .update(cx, |document, _| document.save_to(&path));
        match result {
            Ok(()) => {
                self.project_path = Some(path.clone());
                self.saved_revision = self.document.read(cx).revision;
                self.remember_recent(path);
                if let Err(error) = crate::autosave::clean(
                    self.project_path.as_deref().expect("saved project path"),
                ) {
                    eprintln!("failed to remove autosave backup: {error}");
                }
            }
            Err(error) => eprintln!("failed to save project: {error}"),
        }
        cx.notify();
    }

    fn open_project_with_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_dirty(cx) {
            self.open_project(cx);
            return;
        }
        let response = window.prompt(
            PromptLevel::Warning,
            crate::i18n::t("Unsaved changes will be lost. Continue?"),
            None,
            &[crate::i18n::t("Continue"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(response.await, Ok(0)) {
                this.update(cx, |this, cx| this.open_project(cx)).ok();
            }
        })
        .detach();
    }

    fn open_project(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(crate::i18n::t("Open Free3D project (.f3d)").into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else {
                return;
            };
            let Some(path) = paths.into_iter().find(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("f3d"))
            }) else {
                eprintln!("failed to open project: select an .f3d file");
                return;
            };
            this.update(cx, |this, cx| this.open_project_path_now(path, cx))
                .ok();
        })
        .detach();
    }

    /// Opens one recent project, protecting unsaved work first.
    pub fn open_recent(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.is_dirty(cx) {
            self.open_project_path_now(path, cx);
            return;
        }
        let response = window.prompt(
            PromptLevel::Warning,
            crate::i18n::t("Unsaved changes will be lost. Continue?"),
            None,
            &[crate::i18n::t("Continue"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(response.await, Ok(0)) {
                this.update(cx, |this, cx| this.open_project_path_now(path, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn open_project_path_now(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        match Document::load_from(&path) {
            Ok(document) => {
                self.install_document(document, Some(path.clone()), cx);
                self.remember_recent(path);
            }
            Err(error) => eprintln!("failed to open project: {error}"),
        }
    }

    fn new_project_with_confirmation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_dirty(cx) {
            self.install_document(Document::new(), None, cx);
            return;
        }
        let response = window.prompt(
            PromptLevel::Warning,
            crate::i18n::t("Unsaved changes will be lost. Continue?"),
            None,
            &[crate::i18n::t("Continue"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(response.await, Ok(0)) {
                this.update(cx, |this, cx| {
                    this.install_document(Document::new(), None, cx)
                })
                .ok();
            }
        })
        .detach();
    }

    fn install_document(
        &mut self,
        mut replacement: Document,
        path: Option<std::path::PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let scene_epoch = self.document.read(cx).scene_epoch.wrapping_add(1);
        replacement.scene_epoch = scene_epoch;
        let revision = replacement.revision;
        self.document.update(cx, |document, cx| {
            *document = replacement;
            cx.notify();
        });
        self.project_path = path;
        self.saved_revision = revision;
        self.screen = AppScreen::Workspace;
        self.space = Space::Modeling;
        self.active_tool = None;
        self.open_group = None;
        self.inspection_card = None;
        self.show_views = false;
        self.show_command_search = false;
        self.show_settings = false;
        self.show_nav_presets = false;
        self.renaming_body = None;
        self.renaming_plane = None;
        self.renaming_reference_image = None;
        self.renaming_variable = None;
        self.history_editor = None;
        self.history_editor_subscription = None;
        self.variable_editor = None;
        self.variable_editor_subscription = None;
        self.material_editor = None;
        self.material_editor_subscription = None;
        self.joint_editor = None;
        self.joint_editor_subscription = None;
        self.replay_error = None;
        self.drawing_cache.clear();
        cx.notify();
    }

    /// Writes a dirty project to its deterministic autosave path.
    pub fn autosave_now(&mut self, cx: &mut Context<Self>) {
        if !self.is_dirty(cx) {
            return;
        }
        self.capture_thumbnail(cx);
        let project_path = self.project_path.as_deref();
        let result = self.document.update(cx, |document, _| {
            crate::autosave::write(document, project_path)
        });
        if let Err(error) = result {
            eprintln!("autosave failed: {error}");
        }
    }

    /// Starts the recurring background autosave loop.
    pub fn start_autosave_timer(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(interval) = this.update(cx, |this, _| this.autosave_interval_secs) else {
                    break;
                };
                let delay = if interval == 0 { 60 } else { interval };
                cx.background_executor()
                    .timer(Duration::from_secs(delay))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.autosave_interval_secs != 0 {
                            this.autosave_now(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Prompts for the newest recoverable autosave found at startup.
    pub fn prompt_for_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(recovery) = crate::autosave::discover(&self.recent_files)
            .into_iter()
            .next()
        else {
            return;
        };
        let response = window.prompt(
            PromptLevel::Warning,
            crate::i18n::t("A newer autosave was found. Restore it?"),
            None,
            &[crate::i18n::t("Restore"), crate::i18n::t("Ignore")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(choice) = response.await else {
                return;
            };
            this.update(cx, |this, cx| {
                if choice == 0 {
                    match Document::load_from(&recovery.autosave_path) {
                        Ok(document) => {
                            this.install_document(document, recovery.project_path, cx);
                            this.saved_revision = this.document.read(cx).revision.wrapping_add(1);
                        }
                        Err(error) => eprintln!("failed to restore autosave backup: {error}"),
                    }
                } else if let Err(error) = std::fs::remove_file(&recovery.autosave_path) {
                    eprintln!("failed to delete autosave backup: {error}");
                }
            })
            .ok();
        })
        .detach();
    }

    fn remember_recent(&mut self, path: std::path::PathBuf) {
        self.recent_files.retain(|recent| recent != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(8);
        if let Err(error) = save_recent_files(&self.recent_files) {
            eprintln!("failed to save recent-file list: {error}");
        }
    }

    fn capture_thumbnail(&mut self, cx: &mut Context<Self>) {
        let thumbnail = self
            .viewport
            .read(cx)
            .latest_frame()
            .and_then(|(width, height, bytes)| crate::home::encode_thumbnail(width, height, bytes));
        if thumbnail.is_some() {
            self.document.update(cx, |document, _| {
                document.thumbnail = thumbnail;
            });
        }
    }

    fn refresh_home(&mut self) {
        self.home_designs = crate::home::list_designs(&self.recent_files);
        let current: std::collections::HashSet<_> = self
            .home_designs
            .iter()
            .map(|design| design.path.clone())
            .collect();
        self.home_thumbnails
            .retain(|path, _| current.contains(path));
    }

    fn enter_home_now(&mut self, cx: &mut Context<Self>) {
        self.install_document(Document::new(), None, cx);
        self.screen = AppScreen::Home;
        self.home_menu_path = None;
        self.home_rename_path = None;
        self.home_rename_buffer.clear();
        self.refresh_home();
        cx.notify();
    }

    /// Navigates back to the library using the same dirty-document guard as Open Recent.
    pub fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.is_dirty(cx) {
            self.enter_home_now(cx);
            return;
        }
        let response = window.prompt(
            PromptLevel::Warning,
            crate::i18n::t("Unsaved changes will be lost. Continue?"),
            None,
            &[crate::i18n::t("Continue"), crate::i18n::t("Cancel")],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if matches!(response.await, Ok(0)) {
                this.update(cx, |this, cx| this.enter_home_now(cx)).ok();
            }
        })
        .detach();
    }

    /// Starts an empty untitled design from the library.
    pub(crate) fn new_design(&mut self, cx: &mut Context<Self>) {
        self.install_document(Document::new(), None, cx);
    }

    /// Starts a fresh design and invokes the workspace import flow.
    pub(crate) fn import_from_home(&mut self, cx: &mut Context<Self>) {
        self.install_document(Document::new(), None, cx);
        self.import(cx);
    }

    pub(crate) fn toggle_home_menu(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        if self.home_menu_path.as_ref() == Some(&path) {
            self.home_menu_path = None;
        } else {
            self.home_menu_path = Some(path);
        }
        cx.notify();
    }

    pub(crate) fn begin_home_rename(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.home_rename_buffer = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned();
        self.home_rename_path = Some(path);
        self.home_menu_path = None;
        window.focus(&self.home_focus, cx);
        cx.notify();
    }

    fn commit_home_rename(&mut self, cx: &mut Context<Self>) {
        let Some(old) = self.home_rename_path.take() else {
            return;
        };
        let name = self.home_rename_buffer.trim();
        if name.is_empty() || name.contains(['/', ':']) {
            self.home_rename_buffer.clear();
            cx.notify();
            return;
        }
        let mut new = old.with_file_name(name);
        new.set_extension("f3d");
        if new != old && !new.exists() {
            match std::fs::rename(&old, &new) {
                Ok(()) => {
                    crate::home::rename_recent_paths(&mut self.recent_files, &old, &new);
                    if let Err(error) = save_recent_files(&self.recent_files) {
                        eprintln!("failed to save recent-file list: {error}");
                    }
                    if let Some(thumbnail) = self.home_thumbnails.remove(&old) {
                        self.home_thumbnails.insert(new, thumbnail);
                    }
                }
                Err(error) => eprintln!("failed to rename design: {error}"),
            }
        }
        self.home_rename_buffer.clear();
        self.refresh_home();
        cx.notify();
    }

    pub(crate) fn duplicate_design(
        &mut self,
        path: std::path::PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let destination = crate::home::unique_copy_path(&path, crate::i18n::lang());
        if let Err(error) = std::fs::copy(&path, &destination) {
            eprintln!("failed to duplicate design: {error}");
        } else {
            self.remember_recent(destination);
        }
        self.home_menu_path = None;
        self.refresh_home();
        cx.notify();
    }

    pub(crate) fn trash_design(
        &mut self,
        path: std::path::PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = move_to_trash(&path);
        if let Err(error) = result {
            eprintln!("failed to move design to Trash: {error}");
        } else {
            self.recent_files.retain(|recent| recent != &path);
            if let Err(error) = save_recent_files(&self.recent_files) {
                eprintln!("failed to save recent-file list: {error}");
            }
        }
        self.home_menu_path = None;
        self.refresh_home();
        cx.notify();
    }

    pub(crate) fn reveal_design(
        &mut self,
        path: std::path::PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = opener::reveal(&path) {
            eprintln!("failed to reveal design: {error}");
        }
        self.home_menu_path = None;
        cx.notify();
    }

    /// Handles the home search field and inline file rename.
    pub(crate) fn home_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let buffer = if self.home_rename_path.is_some() {
            &mut self.home_rename_buffer
        } else {
            &mut self.home_query
        };
        if key.eq_ignore_ascii_case("escape") {
            self.home_rename_path = None;
            self.home_rename_buffer.clear();
        } else if key.eq_ignore_ascii_case("enter") && self.home_rename_path.is_some() {
            self.commit_home_rename(cx);
        } else if key.eq_ignore_ascii_case("backspace") {
            buffer.pop();
        } else if !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && let Some(text) = &event.keystroke.key_char
        {
            buffer.extend(text.chars().filter(|character| !character.is_control()));
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn primitive(&self, tool: ToolId) -> Option<PrimitiveKind> {
        Some(match tool {
            ToolId::Box => PrimitiveKind::Box {
                min: dvec3(-25.0, -25.0, 0.0),
                max: dvec3(25.0, 25.0, 50.0),
            },
            ToolId::Cylinder => PrimitiveKind::Cylinder {
                origin: DVec3::ZERO,
                radius: 25.0,
                axis: DVec3::Z,
                height: 50.0,
            },
            ToolId::Sphere => PrimitiveKind::Sphere {
                center: dvec3(0.0, 0.0, 30.0),
                radius: 30.0,
            },
            ToolId::Cone => PrimitiveKind::Cone {
                origin: DVec3::ZERO,
                bottom_radius: 25.0,
                height: 50.0,
            },
            ToolId::Torus => PrimitiveKind::Torus {
                center: dvec3(0.0, 0.0, 10.0),
                major_radius: 40.0,
                minor_radius: 10.0,
            },
            ToolId::Ellipsoid => PrimitiveKind::Ellipsoid {
                center: dvec3(0.0, 0.0, 30.0),
                radii: dvec3(35.0, 25.0, 30.0),
            },
            ToolId::Prism => PrimitiveKind::Prism {
                center: DVec3::ZERO,
                radius: 30.0,
                sides: self.prism_sides,
                height: 50.0,
            },
            ToolId::Wedge => PrimitiveKind::Wedge {
                origin: dvec3(-25.0, -25.0, 0.0),
                dx: 50.0,
                dy: 50.0,
                dz: 40.0,
                top_dx: 15.0,
            },
            _ => return None,
        })
    }

    /// Starts the Items-panel inline rename editor for `id`.
    pub fn begin_rename(
        &mut self,
        id: crate::document::BodyId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .document
            .read(cx)
            .bodies
            .iter()
            .find(|body| body.id == id)
            .map(|body| body.name.clone())
        else {
            return;
        };
        self.renaming_body = Some(id);
        self.renaming_plane = None;
        self.renaming_variable = None;
        self.renaming_reference_image = None;
        self.rename_buffer = name;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Starts the Items-panel inline rename editor for a construction plane.
    pub fn begin_plane_rename(
        &mut self,
        id: crate::document::PlaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .document
            .read(cx)
            .construction_planes
            .iter()
            .find(|plane| plane.id == id)
            .map(|plane| plane.name.clone())
        else {
            return;
        };
        self.renaming_body = None;
        self.renaming_plane = Some(id);
        self.renaming_variable = None;
        self.renaming_reference_image = None;
        self.rename_buffer = name;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Starts inline rename for a reference image.
    pub fn begin_reference_image_rename(
        &mut self,
        id: crate::document::ReferenceImageId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(name) = self
            .document
            .read(cx)
            .reference_images
            .iter()
            .find(|image| image.id == id)
            .map(|image| image.name.clone())
        else {
            return;
        };
        self.renaming_body = None;
        self.renaming_plane = None;
        self.renaming_variable = None;
        self.renaming_reference_image = Some(id);
        self.rename_buffer = name;
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    fn rename_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if self.renaming_body.is_none()
            && self.renaming_plane.is_none()
            && self.renaming_variable.is_none()
            && self.renaming_reference_image.is_none()
            && self.drawing_title_editor.is_none()
            && self.space == Space::Drawing
            && (key.eq_ignore_ascii_case("delete") || key.eq_ignore_ascii_case("backspace"))
        {
            let selected_view = self.drawing_selected_view.take();
            let selected_dim = self.drawing_selected_dim.take();
            if selected_view.is_some() || selected_dim.is_some() {
                self.document.update(cx, |document, cx| {
                    document.drawing.checkpoint();
                    if let Some(id) = selected_view {
                        document
                            .drawing
                            .sheet_mut()
                            .views
                            .retain(|view| view.id != id);
                    } else if let Some(index) = selected_dim
                        && index < document.drawing.sheet().dims.len()
                    {
                        document.drawing.sheet_mut().dims.remove(index);
                    }
                    document.drawing_changed();
                    cx.notify();
                });
                self.drawing_cache.clear();
            }
            cx.stop_propagation();
            return;
        }
        if self.renaming_body.is_none()
            && self.renaming_plane.is_none()
            && self.renaming_variable.is_none()
            && self.renaming_reference_image.is_none()
            && self.drawing_title_editor.is_none()
        {
            if self.space == Space::Drawing && self.drawing_tool == Some(DrawingTool::Detail) {
                if key.eq_ignore_ascii_case("enter") && !self.drawing_numeric_buffer.is_empty() {
                    if let Ok(value) = self.drawing_numeric_buffer.parse::<f64>()
                        && value.is_finite()
                        && value > 0.0
                    {
                        self.drawing_detail_scale = value;
                    }
                    self.drawing_numeric_buffer.clear();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                if !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.control
                    && let Some(text) = &event.keystroke.key_char
                    && text
                        .chars()
                        .all(|character| character.is_ascii_digit() || character == '.')
                {
                    self.drawing_numeric_buffer.push_str(text);
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
            }
            if key.eq_ignore_ascii_case("escape") && self.inspection_card.is_some() {
                self.close_inspection(cx);
                cx.stop_propagation();
                return;
            }
            if event.keystroke.modifiers.platform {
                let command = if key.eq_ignore_ascii_case("v") && event.keystroke.modifiers.alt {
                    Some(AppCommand::ToggleVariablesPanel)
                } else if key.eq_ignore_ascii_case("z") && event.keystroke.modifiers.shift {
                    Some(AppCommand::Redo)
                } else if key.eq_ignore_ascii_case("z") {
                    Some(AppCommand::Undo)
                } else if key.eq_ignore_ascii_case("s") && event.keystroke.modifiers.shift {
                    Some(AppCommand::SaveProjectAs)
                } else if key.eq_ignore_ascii_case("s") {
                    Some(AppCommand::SaveProject)
                } else if key.eq_ignore_ascii_case("o") {
                    Some(AppCommand::OpenProject)
                } else if key.eq_ignore_ascii_case("n") {
                    Some(AppCommand::NewProject)
                } else {
                    None
                };
                if let Some(command) = command {
                    self.dispatch(command, window, cx);
                    cx.stop_propagation();
                    return;
                }
            }
            if key.eq_ignore_ascii_case("x")
                && !event.keystroke.modifiers.platform
                && !event.keystroke.modifiers.control
                && !event.keystroke.modifiers.alt
            {
                self.dispatch(AppCommand::CommandSearch, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if key.eq_ignore_ascii_case("enter") {
            let name = self.rename_buffer.trim().to_string();
            if !name.is_empty() || self.drawing_title_editor.is_some() {
                let body = self.renaming_body;
                let plane = self.renaming_plane;
                let variable = self.renaming_variable;
                let reference_image = self.renaming_reference_image;
                let title_field = self.drawing_title_editor;
                self.document.update(cx, |document, cx| {
                    if let Some(id) = body {
                        document.rename(id, name);
                    } else if let Some(id) = plane {
                        document.rename_construction_plane(id, name);
                    } else if let Some(index) = variable
                        && let Some(expression) = document
                            .variables
                            .get(index)
                            .map(|variable| variable.expr.clone())
                    {
                        document.update_variable(index, name, expression);
                    } else if let Some(id) = reference_image {
                        document.rename_reference_image(id, name);
                    } else if let Some(field) = title_field {
                        document.drawing.checkpoint();
                        let title = &mut document.drawing.sheet_mut().title;
                        match field {
                            DrawingTitleField::ProjectName => title.project_name = name,
                            DrawingTitleField::DrawingNumber => title.drawing_number = name,
                            DrawingTitleField::Scale => title.scale = name,
                            DrawingTitleField::Date => title.date = name,
                            DrawingTitleField::Units => title.units = name,
                            DrawingTitleField::Author => title.author = name,
                        }
                        document.drawing_changed();
                    }
                    cx.notify();
                });
            }
            self.renaming_body = None;
            self.renaming_plane = None;
            self.renaming_variable = None;
            self.renaming_reference_image = None;
            self.drawing_title_editor = None;
        } else if key.eq_ignore_ascii_case("escape") {
            self.renaming_body = None;
            self.renaming_plane = None;
            self.renaming_variable = None;
            self.renaming_reference_image = None;
            self.drawing_title_editor = None;
        } else if key.eq_ignore_ascii_case("backspace") {
            self.rename_buffer.pop();
        } else if !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && let Some(text) = &event.keystroke.key_char
        {
            if self.renaming_variable.is_some() {
                self.rename_buffer
                    .extend(text.chars().filter(|character| {
                        character.is_ascii_alphanumeric() || *character == '_'
                    }));
            } else {
                self.rename_buffer.push_str(text);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Starts editing one title-block value with the shared inline text buffer.
    pub(crate) fn begin_drawing_title_edit(
        &mut self,
        field: DrawingTitleField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = &self.document.read(cx).drawing.sheet().title;
        self.rename_buffer = match field {
            DrawingTitleField::ProjectName => &title.project_name,
            DrawingTitleField::DrawingNumber => &title.drawing_number,
            DrawingTitleField::Scale => &title.scale,
            DrawingTitleField::Date => &title.date,
            DrawingTitleField::Units => &title.units,
            DrawingTitleField::Author => &title.author,
        }
        .clone();
        self.drawing_title_editor = Some(field);
        window.focus(&self.rename_focus, cx);
        cx.notify();
    }

    /// Handles keyboard editing and selection inside command search.
    pub(crate) fn command_search_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let result_count = ui::command_search::filtered_commands(&self.command_query).len();
        if key.eq_ignore_ascii_case("escape") {
            self.close_command_search(window, cx);
        } else if key.eq_ignore_ascii_case("enter") {
            if let Some(command) = ui::command_search::filtered_commands(&self.command_query)
                .get(self.command_highlight)
                .copied()
            {
                self.execute_search_command(command, window, cx);
            }
        } else if key.eq_ignore_ascii_case("up") || key.eq_ignore_ascii_case("arrowup") {
            if result_count > 0 {
                self.command_highlight = self
                    .command_highlight
                    .checked_sub(1)
                    .unwrap_or(result_count - 1);
            }
            cx.notify();
        } else if key.eq_ignore_ascii_case("down") || key.eq_ignore_ascii_case("arrowdown") {
            if result_count > 0 {
                self.command_highlight = (self.command_highlight + 1) % result_count;
            }
            cx.notify();
        } else if key.eq_ignore_ascii_case("backspace") {
            self.command_query.pop();
            self.command_highlight = 0;
            cx.notify();
        } else if !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && let Some(text) = &event.keystroke.key_char
        {
            self.command_query.extend(
                text.chars()
                    .filter(|character| character.is_alphanumeric() || *character == ' '),
            );
            self.command_highlight = 0;
            cx.notify();
        }
        cx.stop_propagation();
    }

    /// Closes command search and returns keyboard focus to the viewport.
    pub(crate) fn close_command_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_command_search = false;
        self.viewport
            .update(cx, |viewport, cx| viewport.focus(window, cx));
        cx.notify();
    }

    /// Executes a registry entry and closes the palette.
    pub(crate) fn execute_search_command(
        &mut self,
        command: SearchCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_command_search(window, cx);
        self.dispatch(command.app_command(), window, cx);
    }

    /// Selects one of the two appearance variants.
    pub fn set_theme_variant(&mut self, dark: bool, cx: &mut Context<Self>) {
        self.theme = if dark { Theme::dark() } else { Theme::light() };
        let theme = self.theme.clone();
        self.viewport.update(cx, |viewport, cx| {
            viewport.set_canvas_theme(theme.canvas, cx);
            viewport.set_theme(theme, cx);
        });
        self.persist_settings();
        cx.notify();
    }

    /// Selects and immediately applies a navigation preset.
    pub fn set_nav_preset(&mut self, preset: NavPreset, cx: &mut Context<Self>) {
        self.nav_preset = preset;
        self.show_nav_presets = false;
        self.viewport
            .update(cx, |viewport, cx| viewport.set_nav_preset(preset, cx));
        self.persist_settings();
        cx.notify();
    }

    /// Toggles one surface analysis, turning the other off.
    pub fn toggle_analysis(&mut self, mode: AnalysisMode, cx: &mut Context<Self>) {
        self.analysis = if self.analysis == mode {
            AnalysisMode::Off
        } else {
            mode
        };
        let analysis = self.analysis;
        self.viewport
            .update(cx, |viewport, cx| viewport.set_analysis(analysis, cx));
        cx.notify();
    }

    /// Applies and persists a user-facing length unit.
    pub fn set_units(&mut self, units: Units, cx: &mut Context<Self>) {
        self.units = units;
        self.viewport
            .update(cx, |viewport, cx| viewport.set_units(units, cx));
        self.persist_settings();
        cx.notify();
    }

    /// Applies and persists the interface language preference.
    pub fn set_language(&mut self, choice: crate::i18n::LangChoice, cx: &mut Context<Self>) {
        self.language = choice;
        crate::i18n::init(choice);
        self.viewport.update(cx, |viewport, cx| {
            viewport.refresh_orientation_labels(cx);
        });
        self.persist_settings();
        cx.notify();
    }

    /// Applies and persists the autosave cadence.
    pub fn set_autosave_interval(&mut self, seconds: u64, cx: &mut Context<Self>) {
        self.autosave_interval_secs = seconds;
        self.persist_settings();
        cx.notify();
    }

    fn persist_settings(&self) {
        if let Err(error) = crate::settings::save(crate::settings::Settings {
            dark_theme: self.theme.is_dark,
            nav_preset: self.nav_preset,
            units: self.units,
            language: self.language,
            autosave_interval_secs: self.autosave_interval_secs,
            tool_banner_collapsed: crate::settings::load().tool_banner_collapsed,
        }) {
            eprintln!("failed to save settings: {error}");
        }
    }

    /// Saves an empty view slot or recalls an already-filled slot.
    pub fn use_saved_view(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(view) = self.saved_views.get(index) {
            self.fov_degrees = view.fov_degrees;
            self.viewport.update(cx, |viewport, cx| {
                viewport.recall_saved_view(view, window, cx)
            });
            self.show_views = false;
        } else {
            let view = self.viewport.read(cx).saved_view();
            self.saved_views.store(index, view);
        }
        cx.notify();
    }

    /// Clears a saved-view slot without changing the camera.
    pub fn clear_saved_view(&mut self, index: usize, cx: &mut Context<Self>) {
        self.saved_views.clear(index);
        cx.notify();
    }

    /// Sets the field of view and forwards it to the camera.
    pub fn set_fov(&mut self, degrees: f32, window: &mut Window, cx: &mut Context<Self>) {
        self.fov_degrees = degrees.clamp(5.0, 90.0);
        let fov = self.fov_degrees;
        self.viewport
            .update(cx, |viewport, cx| viewport.set_fov(fov, window, cx));
        cx.notify();
    }

    /// Begins an FOV scrub at pointer x `origin` (logical pixels).
    pub fn begin_fov_drag(&mut self, origin: f32) {
        self.fov_dragging = true;
        self.fov_last_x = origin;
    }

    /// Advances an active FOV scrub to pointer x `position` over a `track_width`.
    pub fn drag_fov(
        &mut self,
        position: f32,
        track_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.fov_dragging {
            return;
        }
        let delta = (position - self.fov_last_x) / track_width.max(1.0) * 85.0;
        self.fov_last_x = position;
        self.set_fov(self.fov_degrees + delta, window, cx);
    }

    /// Ends an FOV scrub.
    pub fn end_fov_drag(&mut self) {
        self.fov_dragging = false;
    }
}

pub(crate) fn startup_document() -> Document {
    let mut document = Document::new();
    if let Some(scene) = std::env::var_os("FREE3D_DEMO_SCENE") {
        if scene == "9" {
            let base = document.add_primitive(PrimitiveKind::Box {
                min: dvec3(-20.0, -10.0, 0.0),
                max: dvec3(20.0, 10.0, 10.0),
            });
            let base_boss = document.add_primitive(PrimitiveKind::Cylinder {
                origin: dvec3(0.0, -6.0, 10.0),
                radius: 6.0,
                axis: DVec3::Y,
                height: 12.0,
            });
            assert!(document.apply_boolean(BooleanOp::Union, &[base, base_boss]));
            let arm = document.add_primitive(PrimitiveKind::Box {
                min: dvec3(0.0, -4.0, 10.0),
                max: dvec3(42.0, 4.0, 16.0),
            });
            let arm_boss = document.add_primitive(PrimitiveKind::Cylinder {
                origin: dvec3(0.0, -6.0, 10.0),
                radius: 5.0,
                axis: DVec3::Y,
                height: 12.0,
            });
            assert!(document.apply_boolean(BooleanOp::Union, &[arm, arm_boss]));
            let connector_for = |document: &Document, body: BodyId| {
                let shape = &document
                    .bodies
                    .iter()
                    .find(|item| item.id == body)
                    .unwrap()
                    .shape;
                let edge = (0..shape.edge_count().unwrap())
                    .min_by(|&left, &right| {
                        let score = |index| {
                            shape
                                .edge_polyline(index, 0.2)
                                .ok()
                                .and_then(|points| {
                                    (!points.is_empty()).then(|| {
                                        points.iter().copied().sum::<DVec3>() / points.len() as f64
                                    })
                                })
                                .map_or(f64::INFINITY, |center| {
                                    center.distance(DVec3::new(0.0, -6.0, 10.0))
                                })
                        };
                        score(left).total_cmp(&score(right))
                    })
                    .unwrap_or(0) as u32;
                Connector {
                    frame: ConnectorFrame {
                        origin: DVec3::new(0.0, -6.0, 10.0),
                        z: DVec3::Y,
                        x: DVec3::X,
                    },
                    source: ConnectorSource::Edge(edge_ref(shape, edge)),
                    stale: false,
                }
            };
            let joint = Joint {
                id: JointId(0),
                name: "Arm revolute".to_owned(),
                kind: JointKind::Revolute,
                a: (base, connector_for(&document, base)),
                b: (arm, connector_for(&document, arm)),
                value: 0.6,
                value2: 0.0,
                limits: Some((-1.4, 1.4)),
            };
            document.set_grounded(base, true);
            document.add_joint(joint);
            document.selection.items = vec![SelItem::Body(arm)];
            return document;
        }
        if scene == "4" {
            let id = document.add_sketch(SketchPlane::xy());
            let points = [
                glam::DVec2::new(-20.0, -15.0),
                glam::DVec2::new(20.0, -15.0),
                glam::DVec2::new(20.0, 15.0),
                glam::DVec2::new(-20.0, 15.0),
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
                    expr: None,
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
            document.add_sketch_entities_with_constraints(
                id,
                (0..4).map(|index| SketchEntity::Line {
                    a: points[index],
                    b: points[(index + 1) % 4],
                }),
                constraints,
            );
            document.sketches[0].pinned = vec![0, 1];
            document.resolve_sketches();
            let circle = document.add_sketch(SketchPlane::xy());
            document.add_sketch_entities(
                circle,
                [SketchEntity::Circle {
                    center: glam::DVec2::new(60.0, 0.0),
                    radius: 12.0,
                }],
            );
            document.add_sketch_items_with_constraints(
                id,
                [SketchItem::construction(SketchEntity::Line {
                    a: glam::DVec2::new(-35.0, 0.0),
                    b: glam::DVec2::new(35.0, 0.0),
                })],
                [],
            );
            if let Some((polygon, polygon_constraints)) = regular_polygon(
                glam::DVec2::new(-55.0, 0.0),
                glam::DVec2::new(-43.0, 0.0),
                6,
            ) {
                document.add_sketch_primitives(id, polygon, polygon_constraints);
            }
            return document;
        }
        if scene == "8" {
            let sketch = document.add_sketch(SketchPlane::xy());
            let points = [
                glam::DVec2::new(-35.0, -20.0),
                glam::DVec2::new(-10.0, 10.0),
                glam::DVec2::new(15.0, -5.0),
                glam::DVec2::new(40.0, 20.0),
            ];
            document.add_sketch_entities(
                sketch,
                points.windows(2).map(|pair| SketchEntity::Line {
                    a: pair[0],
                    b: pair[1],
                }),
            );
            let _ = document.apply_open_chain_extrude(
                sketch,
                &[0, 1, 2],
                30.0,
                0.0,
                ExtrudeSideMode::OneSided,
            );
            document.sketches[0].visible = true;
            return document;
        }
        let box_id = document.add_primitive(PrimitiveKind::Box {
            min: dvec3(-25.0, -25.0, 0.0),
            max: dvec3(25.0, 25.0, 50.0),
        });
        let cylinder_id = document.add_primitive(PrimitiveKind::Cylinder {
            origin: dvec3(if scene == "3" { 20.0 } else { 70.0 }, 0.0, 0.0),
            radius: 25.0,
            axis: DVec3::Z,
            height: 50.0,
        });
        if scene == "7" {
            document.set_material(
                box_id,
                Material {
                    base_color: [0.82, 0.07, 0.055],
                    metallic: 0.0,
                    roughness: 0.30,
                },
            );
            document.set_material(
                cylinder_id,
                Material {
                    base_color: [0.58, 0.62, 0.66],
                    metallic: 0.92,
                    roughness: 0.38,
                },
            );
        }
        document.selection.items = if scene == "2" {
            let shape = &document.bodies[0].shape;
            let top = (0..shape.face_count().expect("demo box face count"))
                .filter_map(|index| Some((index, shape.face_center_of_mass(index).ok()?)))
                .max_by(|(_, a), (_, b)| a.z.total_cmp(&b.z))
                .map(|(index, _)| index as u32)
                .expect("demo box has faces");
            vec![SelItem::Face(box_id, top)]
        } else if scene == "3" {
            vec![SelItem::Body(box_id), SelItem::Body(cylinder_id)]
        } else {
            vec![SelItem::Body(box_id)]
        };
        if scene == "6" {
            let parent = document
                .drawing
                .add_view(Projection::Front, DVec2::new(72.0, 72.0), 0.5);
            document.drawing.add_derived_view(
                Projection::Front,
                DVec2::new(190.0, 72.0),
                0.5,
                ViewKind::Section {
                    parent_id: parent,
                    line_a: DVec2::new(47.0, 72.0),
                    line_b: DVec2::new(97.0, 72.0),
                    plane_origin: DVec3::new(0.0, 0.0, 25.0),
                    plane_normal: DVec3::Z,
                    label: "A".into(),
                },
            );
            document
                .drawing
                .add_view(Projection::Top, DVec2::new(80.0, 145.0), 0.5);
            document
                .drawing
                .sheet_mut()
                .dims
                .push(crate::drawing::DrawingDim {
                    kind: crate::drawing::DimensionKind::Radius,
                    a: DVec2::new(97.5, 145.0),
                    b: DVec2::new(110.0, 145.0),
                    offset: 7.0,
                    value_mm: 25.0,
                    c: Some(DVec2::new(97.5, 145.0)),
                    d: None,
                });
            document
                .drawing
                .sheet_mut()
                .bom_tables
                .push(crate::drawing::BomTable {
                    at: DVec2::new(177.0, 105.0),
                });
            document
                .drawing
                .sheet_mut()
                .balloons
                .push(crate::drawing::Balloon {
                    view_id: parent,
                    body_id: box_id,
                    anchor: DVec2::new(60.0, 60.0),
                    at: DVec2::new(42.0, 46.0),
                });
        }
    }
    document
}

impl Free3dApp {
    /// Changes workspace and closes chrome that is irrelevant to the destination.
    pub fn set_space(&mut self, space: Space, cx: &mut Context<Self>) {
        if self.space != Space::Visualize && space == Space::Visualize {
            self.grid_before_visualize = Some(self.grid_visible);
            self.grid_visible = false;
            self.selection_filter_before_visualize = Some(self.document.read(cx).selection.filter);
            self.document.update(cx, |document, _| {
                document.selection.filter = SelectionFilter::Body
            });
        } else if self.space == Space::Visualize && space != Space::Visualize {
            self.grid_visible = self.grid_before_visualize.take().unwrap_or(true);
            let filter = self
                .selection_filter_before_visualize
                .take()
                .unwrap_or_default();
            self.document
                .update(cx, |document, _| document.selection.filter = filter);
        }
        self.space = space;
        self.active_tool = None;
        self.open_group = None;
        self.drawing_tool = None;
        self.drawing_pending_view_at = None;
        self.drawing_pending_dim = None;
        self.drawing_pending_section = None;
        self.drawing_pending_angle = None;
        self.drawing_title_editor = None;
        self.drawing_selected_dim = None;
        self.show_items = space == Space::Modeling;
        self.show_history = false;
        self.show_variables = false;
        self.show_views = false;
        let grid_visible = self.grid_visible;
        self.viewport.update(cx, |viewport, cx| {
            viewport.set_visualize(space == Space::Visualize, cx);
            viewport.set_grid_visible_passive(grid_visible, cx);
        });
        cx.notify();
    }

    fn refresh_drawing_cache(&mut self, cx: &Context<Self>) {
        let document = self.document.read(cx);
        let revision = document.scene_epoch;
        if !self
            .drawing_bom_cache
            .as_ref()
            .is_some_and(|(cached, units, _)| *cached == revision && *units == self.units)
        {
            self.drawing_bom_cache = Some((
                revision,
                self.units,
                crate::drawing::bom_rows(&document.bodies, self.units),
            ));
        }
        let views = document
            .drawing
            .sheets
            .iter()
            .flat_map(|sheet| sheet.views.iter().cloned())
            .collect::<Vec<_>>();
        for view in views {
            if self
                .drawing_cache
                .get(&view.id)
                .is_some_and(|(cached, _)| *cached == revision)
            {
                continue;
            }
            let projected = project_visible_bodies(document, &view);
            self.drawing_cache.insert(view.id, (revision, projected));
        }
    }

    /// Places a view after projecting it once to choose a standard auto-fit scale.
    pub fn place_drawing_view(
        &mut self,
        projection: Projection,
        at: DVec2,
        cx: &mut Context<Self>,
    ) {
        let document = self.document.read(cx);
        let revision = document.scene_epoch;
        let projected = project_visible_bodies_projection(document, projection);
        let scale = crate::drawing::auto_fit_scale(projected.size, DVec2::new(110.0, 75.0));
        let id = self.document.update(cx, |document, cx| {
            let id = document.drawing.add_view(projection, at, scale);
            document.drawing_changed();
            cx.notify();
            id
        });
        self.drawing_cache.insert(id, (revision, projected));
        self.drawing_selected_view = Some(id);
        self.drawing_pending_view_at = None;
        cx.notify();
    }

    pub(crate) fn drawing_projections(&self, cx: &Context<Self>) -> HashMap<u64, ProjectedView> {
        self.document
            .read(cx)
            .drawing
            .sheets
            .iter()
            .flat_map(|sheet| &sheet.views)
            .filter_map(|view| {
                self.drawing_cache
                    .get(&view.id)
                    .map(|(_, projected)| (view.id, projected.clone()))
            })
            .collect()
    }

    pub(crate) fn drawing_bom_rows(&self) -> &[crate::drawing::BomRow] {
        self.drawing_bom_cache
            .as_ref()
            .map_or(&[], |(_, _, rows)| rows)
    }

    /// Begins direct scrubbing of the 0..1 exploded factor.
    pub(crate) fn begin_exploded_drag(&mut self, origin: f32) {
        self.exploded_dragging = true;
        self.exploded_last_x = origin;
    }

    pub(crate) fn drag_exploded(
        &mut self,
        position: f32,
        width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.exploded_dragging {
            return;
        }
        let delta = (position - self.exploded_last_x) / width.max(1.0);
        self.exploded_last_x = position;
        self.set_exploded_factor(self.exploded_factor + delta, window, cx);
    }

    pub(crate) fn end_exploded_drag(&mut self) {
        self.exploded_dragging = false;
    }

    pub(crate) fn set_exploded_factor(
        &mut self,
        factor: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.exploded_factor = factor.clamp(0.0, 1.0);
        self.active_modes[Self::mode_index(ModeChip::Exploded)] = self.exploded_factor > 0.0;
        let factor = self.exploded_factor;
        self.viewport.update(cx, |viewport, cx| {
            viewport.set_exploded_factor(factor, window, cx)
        });
        cx.notify();
    }
}

fn project_visible_bodies(
    document: &Document,
    view: &crate::drawing::DrawingView,
) -> ProjectedView {
    let projected = project_visible_bodies_with(document, |shape| match &view.kind {
        ViewKind::Section {
            plane_origin,
            plane_normal,
            ..
        } => crate::drawing::shape_section_hlr(
            shape,
            *plane_origin,
            *plane_normal,
            view.view_dir.unwrap_or(*plane_normal),
        ),
        _ => crate::drawing::shape_hlr_dir(
            shape,
            view.view_dir.unwrap_or(view.projection.view_dir()),
        ),
    });
    if let ViewKind::Detail {
        parent_id,
        center,
        radius,
        ..
    } = &view.kind
        && let Some(parent) = document
            .drawing
            .sheets
            .iter()
            .flat_map(|sheet| &sheet.views)
            .find(|candidate| candidate.id == *parent_id)
    {
        let model_center = projected.center + (*center - parent.at) / parent.scale;
        return crate::drawing::clip_projected_circle(
            projected,
            model_center,
            *radius / parent.scale,
        );
    }
    projected
}

fn project_visible_bodies_projection(document: &Document, projection: Projection) -> ProjectedView {
    project_visible_bodies_with(document, |shape| {
        crate::drawing::shape_hlr(shape, projection)
    })
}

fn project_visible_bodies_with(
    document: &Document,
    project: impl Fn(&Shape) -> Result<ProjectedView, String>,
) -> ProjectedView {
    let mut shapes = Vec::new();
    let mut tagged = Vec::new();
    for body in document.bodies.iter().filter(|body| body.visible) {
        shapes.push((*body.shape).clone());
        match project(&body.shape) {
            Ok(projected) => tagged.push((body.id, projected)),
            Err(error) => eprintln!("HLR projection failed for {}: {error}", body.name),
        }
    }
    if shapes.is_empty() {
        return ProjectedView::default();
    }
    let shape = if shapes.len() == 1 {
        shapes.pop().expect("one visible shape")
    } else {
        match Shape::compound(shapes) {
            Ok(shape) => shape,
            Err(error) => {
                eprintln!("HLR compound failed: {error}");
                return ProjectedView::default();
            }
        }
    };
    let mut projected = project(&shape).unwrap_or_else(|error| {
        eprintln!("HLR projection failed: {error}");
        ProjectedView::default()
    });
    let source_for = |line: &[DVec2], hidden: bool| {
        let sample = line.get(line.len() / 2).copied()?;
        tagged
            .iter()
            .flat_map(|(body, view)| {
                let lines = if hidden { &view.hidden } else { &view.visible };
                lines.iter().map(move |candidate| {
                    let distance = candidate
                        .windows(2)
                        .map(|edge| {
                            let segment = edge[1] - edge[0];
                            let t = if segment.length_squared() <= f64::EPSILON {
                                0.0
                            } else {
                                ((sample - edge[0]).dot(segment) / segment.length_squared())
                                    .clamp(0.0, 1.0)
                            };
                            sample.distance(edge[0] + segment * t)
                        })
                        .fold(f64::INFINITY, f64::min);
                    (distance, *body)
                })
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, body)| body)
    };
    projected.visible_sources = projected
        .visible
        .iter()
        .map(|line| source_for(line, false))
        .collect();
    projected.hidden_sources = projected
        .hidden
        .iter()
        .map(|line| source_for(line, true))
        .collect();
    projected
}

impl Render for Free3dApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.screen == AppScreen::Home {
            window.set_window_title("Free3D");
            window.focus(&self.home_focus, cx);
            self.refresh_home();
            let query = self.home_query.trim().to_lowercase();
            for design in self.home_designs.iter().filter(|design| {
                query.is_empty()
                    || design
                        .path
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .is_some_and(|name| name.to_lowercase().contains(&query))
            }) {
                self.home_thumbnails
                    .entry(design.path.clone())
                    .or_insert_with(|| crate::home::decode_thumbnail(&design.path));
            }
            return crate::home::render(self, cx).into_any_element();
        }
        window.set_window_title("Free3D");
        if self.space == Space::Drawing {
            self.refresh_drawing_cache(cx);
        }
        let this = &*self;
        div()
            .relative()
            .size_full()
            .track_focus(&self.rename_focus)
            .on_key_down(cx.listener(Self::rename_key_down))
            .text_color(this.theme.text)
            .when(this.space != Space::Drawing, |root| {
                root.child(this.viewport.clone())
            })
            .when(this.space == Space::Drawing, |root| {
                root.child(ui::drawing_canvas::render(this, window, cx))
            })
            .child(ui::top_bar::render(this, cx))
            .child(ui::tool_strip::render(this, cx))
            .when(this.space == Space::Modeling, |root| {
                root.children(ui::adaptive_menu::render(this, cx))
            })
            .child(ui::view_cluster::render(this, cx))
            .children(ui::command_search::render(this, cx))
            .children(ui::inspection_card::render(this, cx))
            .when(this.space == Space::Modeling, |root| {
                root.children(ui::constraints_panel::render(this, cx))
            })
            .into_any_element()
    }
}

fn move_to_trash(path: &std::path::Path) -> Result<(), String> {
    trash::delete(path).map_err(|error| error.to_string())
}

fn recent_file_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|config| config.join("Free3D/recent.json"))
}

fn load_recent_files() -> Vec<std::path::PathBuf> {
    let Some(path) = recent_file_path() else {
        return Vec::new();
    };
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<std::path::PathBuf>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .take(8)
        .collect()
}

fn save_recent_files(paths: &[std::path::PathBuf]) -> Result<(), String> {
    let path = recent_file_path().ok_or_else(|| crate::i18n::t("HOME is not set").to_owned())?;
    let parent = path.parent().expect("recent.json has a parent directory");
    std::fs::create_dir_all(parent).map_err(|error| {
        crate::i18n::tr2(
            "Could not create {}: {}",
            &parent.display().to_string(),
            &error.to_string(),
        )
    })?;
    let json = serde_json::to_vec(paths).map_err(|error| error.to_string())?;
    std::fs::write(&path, json).map_err(|error| {
        crate::i18n::tr2(
            "Could not write {}: {}",
            &path.display().to_string(),
            &error.to_string(),
        )
    })
}

#[cfg(test)]
mod f7_tests {
    use super::*;
    use crate::{
        assembly::{Connector, ConnectorFrame, ConnectorSource, Joint},
        document::PointId,
    };

    #[test]
    fn joint_editor_value_commits_set_joint_value_and_replays() {
        let mut document = Document::new();
        let base = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::ZERO,
            max: DVec3::ONE,
        });
        let moving = document.add_primitive(PrimitiveKind::Box {
            min: DVec3::new(2.0, 0.0, 0.0),
            max: DVec3::new(3.0, 1.0, 1.0),
        });
        let connector = || Connector {
            frame: ConnectorFrame {
                origin: DVec3::ZERO,
                z: DVec3::Z,
                x: DVec3::X,
            },
            source: ConnectorSource::Point(PointId(u64::MAX)),
            stale: false,
        };
        document.set_grounded(base, true);
        let id = document
            .add_joint(Joint {
                id: JointId(0),
                name: "Slider".into(),
                kind: JointKind::Slider,
                a: (base, connector()),
                b: (moving, connector()),
                value: 0.0,
                value2: 0.0,
                limits: None,
            })
            .unwrap();
        let internal = joint_editor_internal_value(JointKind::Slider, Units::Centimeter, 2.5);
        assert!(document.set_joint_value(id, internal, 0.0));
        assert!(
            matches!(document.history.last().map(|step| &step.op), Some(crate::history::HistoryOp::SetJointValue { id: replayed, value, .. }) if *replayed == id && (*value - 25.0).abs() < 1.0e-9)
        );
        let replayed = crate::history::replay(&document.history).unwrap();
        assert!((replayed.joints[0].value - 25.0).abs() < 1.0e-9);
    }
}
