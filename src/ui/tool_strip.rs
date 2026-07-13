//! Left tool rail: Spaces and Tools cards, modeling panels, and interaction modes.

use gpui::{
    Anchor, AnchoredPositionMode, Context, ElementId, FontWeight, MouseButton, Rgba, Stateful,
    anchored, deferred, div, point, prelude::*, px,
};

use crate::{
    app::{DrawingTool, Free3dApp, MaterialNumericField, Space},
    commands::{AppCommand, ToolGroup, ToolId},
    document::{Material, SelItem, rgb_to_hsl},
    theme::Theme,
    ui::{self, glyph},
};

/// Left inset shared by every left-anchored floating group.
pub const LEFT_INSET: f32 = 12.0;
/// Width of the Spaces / Tools cards and the panels stacked below them.
pub const LEFT_WIDTH: f32 = 200.0;
/// Fixed height of one icon + label rail row.
const ROW_H: f32 = 34.0;

/// Y coordinate (logical px) where the left rail's cards begin.
pub fn rail_top(theme: &Theme) -> f32 {
    theme.unit * 6.0 + theme.control + 6.0
}

/// Builds the floating left rail anchored to the left edge.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    div()
        .absolute()
        .top(px(rail_top(theme)))
        .bottom(theme.space(3.0))
        .left(px(LEFT_INSET))
        .w(px(LEFT_WIDTH))
        .flex()
        .flex_col()
        .gap(theme.space(2.0))
        .child(spaces_card(app, cx))
        .child(if app.space == Space::Drawing {
            drawing_tools_card(app, cx).into_any_element()
        } else if app.space == Space::Visualize {
            materials_card(app, cx).into_any_element()
        } else {
            tools_card(app, cx).into_any_element()
        })
        .when(app.space == Space::Modeling, |column| {
            column
                .child(ui::panels::render(app, cx))
                .child(ui::modes::render(app, cx))
        })
}

/// The Spaces card: workspace entries with the active one highlighted.
fn spaces_card(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    ui::surface(theme)
        .w(px(LEFT_WIDTH))
        .flex()
        .flex_col()
        .p(theme.space(1.5))
        .child(
            rail_row(
                theme,
                "space-modeling",
                "modeling",
                crate::i18n::t("Modeling"),
                None,
                app.space == Space::Modeling,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.set_space(Space::Modeling, cx))),
        )
        .child(
            rail_row(
                theme,
                "space-visualize",
                "visualize",
                crate::i18n::t("Visualize"),
                None,
                app.space == Space::Visualize,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.set_space(Space::Visualize, cx))),
        )
        .child(
            rail_row(
                theme,
                "space-draw",
                "draw",
                crate::i18n::t("Drawing"),
                Some("⇧⌘\\"),
                app.space == Space::Drawing,
            )
            .on_click(cx.listener(|this, _, _window, cx| this.set_space(Space::Drawing, cx))),
        )
        .child(
            rail_row(
                theme,
                "space-items",
                "items",
                crate::i18n::t("Items"),
                None,
                false,
            )
            .when(app.show_items, |row| row.bg(theme.active))
            .when(app.space == Space::Modeling, |row| {
                row.on_click(cx.listener(|this, _, window, cx| {
                    this.dispatch(AppCommand::ToggleItemsPanel, window, cx)
                }))
            }),
        )
        .child(
            rail_row(
                theme,
                "space-variables",
                "items",
                crate::i18n::t("Variables"),
                Some("⌘⌥V"),
                false,
            )
            .when(app.show_variables, |row| row.bg(theme.active))
            .on_click(cx.listener(|this, _, window, cx| {
                this.dispatch(AppCommand::ToggleVariablesPanel, window, cx)
            })),
        )
        .when(app.space == Space::Drawing, |card| {
            card.child(drawing_tool_row(
                theme,
                "drawing-section",
                "section",
                crate::i18n::t("Section View"),
                DrawingTool::Section,
                app,
                cx,
            ))
            .child(drawing_tool_row(
                theme,
                "drawing-detail",
                "zoom-in",
                crate::i18n::t("Detail"),
                DrawingTool::Detail,
                app,
                cx,
            ))
            .child(drawing_tool_row(
                theme,
                "drawing-radius",
                "radius",
                crate::i18n::t("Radius"),
                DrawingTool::Radius,
                app,
                cx,
            ))
            .child(drawing_tool_row(
                theme,
                "drawing-diameter",
                "circle",
                crate::i18n::t("Diameter"),
                DrawingTool::Diameter,
                app,
                cx,
            ))
            .child(drawing_tool_row(
                theme,
                "drawing-angle",
                "angle",
                crate::i18n::t("Angle"),
                DrawingTool::Angle,
                app,
                cx,
            ))
            .child(drawing_tool_row(
                theme,
                "drawing-bom",
                "items",
                crate::i18n::t("Parts List"),
                DrawingTool::Bom,
                app,
                cx,
            ))
            .child(drawing_tool_row(
                theme,
                "drawing-balloon",
                "circle",
                crate::i18n::t("Balloon"),
                DrawingTool::Balloon,
                app,
                cx,
            ))
        })
}

fn material_color(color: [f32; 3]) -> gpui::Hsla {
    Rgba {
        r: color[0],
        g: color[1],
        b: color[2],
        a: 1.0,
    }
    .into()
}

/// Visualize-space body list and live material editor.
fn materials_card(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let document = app.document.read(cx);
    let selected_id = document
        .selection
        .items
        .iter()
        .find_map(|item| item.body_id());
    let selected = selected_id.and_then(|id| document.bodies.iter().find(|body| body.id == id));
    let mut card = ui::surface(theme)
        .w(px(LEFT_WIDTH))
        .flex()
        .flex_col()
        .p(theme.space(1.5))
        .gap(theme.space(1.0))
        .child(
            div()
                .px(theme.space(1.5))
                .text_size(px(theme.text_sm))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_faint)
                .child(crate::i18n::t("Materials")),
        );
    for body in &document.bodies {
        let id = body.id;
        card = card.child(
            div()
                .id(("material-body", id.0))
                .flex()
                .items_center()
                .gap(theme.space(2.0))
                .px(theme.space(1.5))
                .h(px(30.0))
                .rounded(px(theme.radius_control))
                .when(selected_id == Some(id), |row| row.bg(theme.accent_wash))
                .hover(|row| row.bg(theme.hover))
                .cursor_pointer()
                .child(
                    div()
                        .size(px(12.0))
                        .rounded_full()
                        .bg(material_color(body.material.base_color))
                        .border_1()
                        .border_color(theme.border_strong),
                )
                .child(body.name.clone())
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.document.update(cx, |document, cx| {
                        document.selection.apply(SelItem::Body(id), false);
                        cx.notify();
                    });
                    cx.notify();
                })),
        );
    }
    if let Some(body) = selected {
        card = card.child(material_editor(app, body.id, body.material, cx));
    } else {
        card = card.child(
            div()
                .px(theme.space(1.5))
                .py(theme.space(2.0))
                .text_size(px(theme.text_sm))
                .text_color(theme.text_muted)
                .child(crate::i18n::t("Select a body to edit its material")),
        );
    }
    card
}

fn material_editor(
    app: &Free3dApp,
    body: crate::document::BodyId,
    material: Material,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    let presets = [
        (crate::i18n::t("Original"), Material::default()),
        (
            crate::i18n::t("Metal"),
            Material {
                metallic: 0.92,
                roughness: 0.28,
                ..material
            },
        ),
        (
            crate::i18n::t("Plastic"),
            Material {
                metallic: 0.0,
                roughness: 0.32,
                ..material
            },
        ),
        (
            crate::i18n::t("Glass"),
            Material {
                metallic: 0.08,
                roughness: 0.06,
                ..material
            },
        ),
        (
            crate::i18n::t("Rubber"),
            Material {
                metallic: 0.0,
                roughness: 0.90,
                ..material
            },
        ),
    ];
    let colors = [
        [0.86, 0.16, 0.13],
        [0.95, 0.43, 0.10],
        [0.94, 0.73, 0.12],
        [0.20, 0.62, 0.31],
        [0.12, 0.50, 0.76],
        [0.24, 0.29, 0.72],
        [0.57, 0.24, 0.67],
        [0.78, 0.78, 0.75],
    ];
    let mut preset_row = div().flex().flex_wrap().gap(px(3.0));
    for (index, (label, preset)) in presets.into_iter().enumerate() {
        preset_row = preset_row.child(
            div()
                .id(("material-preset", index))
                .px(theme.space(1.5))
                .py(px(3.0))
                .rounded(px(5.0))
                .bg(theme.well)
                .text_size(px(theme.text_sm))
                .cursor_pointer()
                .hover(|button| button.bg(theme.hover))
                .child(label)
                .on_click(
                    cx.listener(move |this, _, _window, cx| this.apply_material(body, preset, cx)),
                ),
        );
    }
    let mut swatches = div().flex().gap(px(5.0));
    for (index, color) in colors.into_iter().enumerate() {
        swatches = swatches.child(
            div()
                .id(("material-color", index))
                .size(px(17.0))
                .rounded(px(4.0))
                .bg(material_color(color))
                .border_1()
                .border_color(theme.border_strong)
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.apply_material(
                        body,
                        Material {
                            base_color: color,
                            ..material
                        },
                        cx,
                    )
                })),
        );
    }
    let hsl = rgb_to_hsl(material.base_color);
    let mut custom = div().flex().flex_col().gap(px(3.0));
    for (field, label, value, suffix) in [
        (
            MaterialNumericField::Hue,
            crate::i18n::t("Hue"),
            hsl[0],
            "°",
        ),
        (
            MaterialNumericField::Saturation,
            crate::i18n::t("Saturation"),
            hsl[1],
            "%",
        ),
        (
            MaterialNumericField::Lightness,
            crate::i18n::t("Lightness"),
            hsl[2],
            "%",
        ),
    ] {
        let editor = app
            .material_editor
            .as_ref()
            .filter(|(id, active, _)| *id == body && *active == field)
            .map(|(_, _, editor)| editor.clone());
        custom = custom.child(
            div()
                .id(("material-hsl", field as usize))
                .flex()
                .items_center()
                .justify_between()
                .px(theme.space(1.5))
                .py(px(3.0))
                .rounded(px(5.0))
                .bg(theme.well)
                .cursor_pointer()
                .child(label)
                .child(format!("{value:.0}{suffix}"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.begin_material_edit(body, field, window, cx)
                }))
                .when_some(editor, |row, editor| row.child(editor)),
        );
    }
    div()
        .mt(theme.space(1.0))
        .pt(theme.space(2.0))
        .border_t_1()
        .border_color(theme.border)
        .flex()
        .flex_col()
        .gap(theme.space(1.5))
        .child(preset_row)
        .child(swatches)
        .child(custom)
}

fn drawing_tools_card(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let hidden = app
        .drawing_selected_view
        .and_then(|id| {
            app.document
                .read(cx)
                .drawing
                .sheet()
                .views
                .iter()
                .find(|view| view.id == id)
                .map(|view| view.show_hidden)
        })
        .unwrap_or(false);
    ui::surface(theme)
        .w(px(LEFT_WIDTH))
        .flex()
        .flex_col()
        .p(theme.space(1.5))
        .child(
            rail_row(
                theme,
                "drawing-view",
                "views",
                crate::i18n::t("View"),
                None,
                app.drawing_tool == Some(DrawingTool::View),
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                this.drawing_tool =
                    (this.drawing_tool != Some(DrawingTool::View)).then_some(DrawingTool::View);
                this.drawing_pending_dim = None;
                cx.notify();
            })),
        )
        .child(
            rail_row(
                theme,
                "drawing-dimension",
                "measure",
                crate::i18n::t("Label"),
                None,
                app.drawing_tool == Some(DrawingTool::Dimension),
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                this.drawing_tool = (this.drawing_tool != Some(DrawingTool::Dimension))
                    .then_some(DrawingTool::Dimension);
                this.drawing_pending_view_at = None;
                cx.notify();
            })),
        )
        .child(
            rail_row(
                theme,
                "drawing-hidden",
                "display",
                crate::i18n::t("Hidden Lines"),
                None,
                hidden,
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                if let Some(id) = this.drawing_selected_view {
                    this.document.update(cx, |document, cx| {
                        document.drawing.checkpoint();
                        if let Some(view) = document
                            .drawing
                            .sheet_mut()
                            .views
                            .iter_mut()
                            .find(|view| view.id == id)
                        {
                            view.show_hidden = !view.show_hidden;
                            document.drawing_changed();
                            cx.notify();
                        }
                    });
                }
                cx.notify();
            })),
        )
        .child(
            rail_row(
                theme,
                "drawing-centerlines",
                "center",
                crate::i18n::t("Centerline"),
                None,
                app.drawing_selected_view
                    .and_then(|id| {
                        app.document
                            .read(cx)
                            .drawing
                            .sheet()
                            .views
                            .iter()
                            .find(|v| v.id == id)
                    })
                    .is_some_and(|v| v.show_centerlines),
            )
            .on_click(cx.listener(|this, _, _window, cx| {
                if let Some(id) = this.drawing_selected_view {
                    this.document.update(cx, |document, cx| {
                        document.drawing.checkpoint();
                        if let Some(view) = document
                            .drawing
                            .sheet_mut()
                            .views
                            .iter_mut()
                            .find(|v| v.id == id)
                        {
                            view.show_centerlines = !view.show_centerlines;
                        }
                        document.drawing_changed();
                        cx.notify();
                    });
                }
            })),
        )
}

fn drawing_tool_row(
    theme: &crate::theme::Theme,
    id: &'static str,
    icon: &'static str,
    label: &'static str,
    tool: DrawingTool,
    app: &Free3dApp,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    rail_row(theme, id, icon, label, None, app.drawing_tool == Some(tool)).on_click(cx.listener(
        move |this, _, _window, cx| {
            this.drawing_tool = (this.drawing_tool != Some(tool)).then_some(tool);
            this.drawing_pending_view_at = None;
            this.drawing_pending_dim = None;
            this.drawing_pending_section = None;
            this.drawing_pending_angle = None;
            cx.notify();
        },
    ))
}

/// The Tools card: command search plus the four tool-group flyouts.
fn tools_card(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut card = ui::surface(theme)
        .w(px(LEFT_WIDTH))
        .flex()
        .flex_col()
        .p(theme.space(1.5))
        .child(
            rail_row(
                theme,
                "search",
                "search",
                crate::i18n::t("Search"),
                Some("⌘F"),
                false,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.dispatch(AppCommand::CommandSearch, window, cx)
            })),
        );
    for group in ToolGroup::ALL {
        card = card.child(group_entry(app, group, cx));
    }
    card
}

/// A shared icon + label row with an optional shortcut chip.
fn rail_row(
    theme: &Theme,
    id: impl Into<ElementId>,
    icon: &str,
    label: &'static str,
    shortcut: Option<&'static str>,
    active: bool,
) -> Stateful<gpui::Div> {
    let fg = if active {
        theme.accent
    } else {
        theme.text_muted
    };
    let label_color = if active { theme.accent } else { theme.text };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.space(1.5))
        .h(px(ROW_H))
        .px(theme.space(1.5))
        .rounded(px(theme.radius_control))
        .when(active, |row| row.bg(theme.accent_wash))
        .hover(|s| s.bg(theme.hover))
        .active(|s| s.bg(theme.active))
        .cursor_pointer()
        .child(
            div()
                .w(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .child(glyph(theme, icon).text_color(fg)),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(theme.text_md))
                .text_color(label_color)
                .child(label),
        )
        .when_some(shortcut, |row, shortcut| {
            row.child(shortcut_chip(theme, shortcut))
        })
}

/// A single tool-group row, wrapping its flyout when open.
fn group_entry(app: &Free3dApp, group: ToolGroup, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let active =
        app.open_group == Some(group) || app.active_tool.is_some_and(|tool| group.contains(tool));
    let is_open = app.open_group == Some(group);

    div().relative().child(
        rail_row(
            theme,
            ("tool-group", group as usize),
            group.icon(),
            group.label(),
            None,
            active,
        )
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.open_group = if this.open_group == Some(group) {
                None
            } else {
                Some(group)
            };
            cx.notify();
        }))
        .when(is_open, |button| button.child(flyout(app, group, cx))),
    )
}

/// The flyout panel listing a group's tools, floating to the rail's right.
fn flyout(app: &Free3dApp, group: ToolGroup, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut panel = ui::surface_elevated(theme)
        .w(px(232.0))
        .flex()
        .flex_col()
        .p(theme.space(1.5))
        .gap(px(1.0))
        .on_mouse_down_out(cx.listener(|this, _, _window, cx| {
            this.open_group = None;
            cx.notify();
        }))
        .child(
            div()
                .px(theme.space(1.5))
                .pt(theme.space(0.5))
                .pb(theme.space(1.5))
                .text_size(px(theme.text_sm))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_faint)
                .child(group.label()),
        );

    for &tool in group.tools() {
        panel = panel.child(tool_row(app, tool, cx));
    }

    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .position(point(px(LEFT_WIDTH - 4.0), px(0.0)))
            .position_mode(AnchoredPositionMode::Local)
            .snap_to_window_with_margin(px(8.0))
            .child(panel),
    )
    .priority(1)
}

/// One selectable tool row inside a flyout.
fn tool_row(app: &Free3dApp, tool: ToolId, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let active = app.active_tool == Some(tool);
    let fg = if active {
        theme.accent
    } else {
        theme.text_muted
    };

    div()
        .id(("tool", tool as usize))
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.space(2.0))
        .px(theme.space(1.5))
        .py(theme.space(1.5))
        .rounded(px(theme.radius_control))
        .text_color(theme.text)
        .when(active, |row| row.bg(theme.accent_wash))
        .hover(|s| s.bg(theme.hover))
        .active(|s| s.bg(theme.active))
        .cursor_pointer()
        .child(glyph(theme, tool.icon()).text_color(fg))
        .child(
            div()
                .flex_1()
                .text_size(px(theme.text_md))
                .child(tool.label()),
        )
        .when_some(tool.shortcut(), |row, shortcut| {
            row.child(shortcut_chip(theme, shortcut))
        })
        .when(tool == ToolId::Prism, |row| {
            row.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("prism-sides-minus")
                            .px_1()
                            .rounded_sm()
                            .bg(theme.well)
                            .child("−")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.prism_sides = this.prism_sides.saturating_sub(1).max(3);
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(theme.text_sm))
                            .child(crate::i18n::tr1("{} sides", &app.prism_sides.to_string())),
                    )
                    .child(
                        div()
                            .id("prism-sides-plus")
                            .px_1()
                            .rounded_sm()
                            .bg(theme.well)
                            .child("+")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.prism_sides = (this.prism_sides + 1).min(24);
                                    cx.notify();
                                }),
                            ),
                    ),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            this.dispatch(AppCommand::ActivateTool(tool), window, cx)
        }))
}

fn shortcut_chip(theme: &Theme, shortcut: &str) -> impl IntoElement {
    div()
        .px(theme.space(1.5))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(theme.well)
        .border_1()
        .border_color(theme.border)
        .text_color(theme.text_muted)
        .text_size(px(theme.text_sm))
        .font_weight(FontWeight::MEDIUM)
        .child(shortcut.to_string())
}
