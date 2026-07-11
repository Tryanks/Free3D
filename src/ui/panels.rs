//! Items and editable parametric History floating panels.

use gpui::{Context, FontWeight, Hsla, Rgba, SharedString, div, prelude::*, px};

use crate::{
    app::{Free3dApp, HistoryNumericField},
    assembly::JointKind,
    document::{AxisId, BodyId, PlaneId, PointId, ReferenceImageId, SelItem},
    history::{HistoryOp, HistoryStep},
    sketch::SketchId,
    theme::Theme,
    ui::{self, glyph, tool_strip},
};

/// A row in the Items panel (body / sketch / plane).
#[derive(Clone, Copy)]
pub enum ItemKind {
    /// BRep body.
    Body(BodyId),
    /// Plane-local sketch.
    Sketch(SketchId),
    /// Named construction plane.
    Plane(PlaneId),
    /// Named construction axis.
    Axis(AxisId),
    /// Named construction point.
    Point(PointId),
    /// Embedded planar raster reference.
    ReferenceImage(ReferenceImageId),
}

/// Presentation data for one Items row.
pub struct ItemRow {
    /// Backing document item.
    pub kind: ItemKind,
    /// Display name.
    pub name: SharedString,
    /// Swatch colour dot.
    pub color: Hsla,
    /// Whether the item is currently visible.
    pub visible: bool,
    /// Whether the body is selected directly or through a subshape.
    pub selected: bool,
    /// Whether this row currently owns the inline rename editor.
    pub editing: bool,
    /// Whether a body is anchored by the assembly solver.
    pub grounded: bool,
}

/// Builds the stacked left-side panels below the tool rail.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let left = px(tool_strip::LEFT_INSET);
    let top = px(tool_strip::panels_top(theme));

    div()
        .absolute()
        .top(top)
        .left(left)
        .flex()
        .flex_col()
        .gap(theme.space(2.0))
        .when(app.show_items, |col| col.child(items_panel(app, cx)))
        .when(app.show_history, |col| col.child(history_panel(app, cx)))
        .when(app.show_variables, |col| {
            col.child(variables_panel(app, cx))
        })
}

fn variables_panel(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let document = app.document.read(cx);
    let mut list = div()
        .id("variable-rows")
        .flex()
        .flex_col()
        .gap(px(1.0))
        .px(theme.space(1.0))
        .max_h(px(220.0))
        .overflow_y_scroll();
    for (index, variable) in document.variables.iter().enumerate() {
        let editing_name = app.renaming_variable == Some(index);
        let name = if editing_name {
            app.rename_buffer.clone()
        } else {
            variable.name.clone()
        };
        let expression = variable.expr.clone();
        let value = variable.value;
        let error = variable.error.clone();
        let editor = app
            .variable_editor
            .as_ref()
            .filter(|(editing, _)| *editing == index)
            .map(|(_, editor)| editor.clone());
        let group = format!("variable-row-{index}");
        list = list.child(
            div()
                .id(("variable", index))
                .group(group.clone())
                .px(theme.space(1.0))
                .py(theme.space(1.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .rounded(px(theme.radius_control))
                .hover(|row| row.bg(theme.hover))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(theme.space(1.0))
                        .child(
                            div()
                                .id(("variable-name", index))
                                .w(px(58.0))
                                .px(px(3.0))
                                .when(editing_name, |field| {
                                    field.border_1().border_color(theme.accent)
                                })
                                .text_size(px(theme.text_sm))
                                .text_color(theme.text)
                                .cursor_pointer()
                                .child(name)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_variable_rename(index, window, cx);
                                })),
                        )
                        .child(
                            div()
                                .id(("variable-expression", index))
                                .flex_1()
                                .min_w(px(58.0))
                                .px(px(4.0))
                                .rounded(px(4.0))
                                .bg(theme.well)
                                .text_size(px(theme.text_sm))
                                .text_color(theme.text_muted)
                                .cursor_pointer()
                                .child(expression)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.begin_variable_expression_edit(index, window, cx);
                                })),
                        )
                        .child(
                            div()
                                .w(px(54.0))
                                .text_right()
                                .text_size(px(theme.text_sm))
                                .text_color(theme.accent)
                                .child(format!("{value:.3}")),
                        )
                        .child(
                            div()
                                .id(("delete-variable", index))
                                .invisible()
                                .group_hover(group, |button| button.visible())
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.0))
                                .text_color(theme.text_muted)
                                .hover(|button| button.bg(theme.active).text_color(theme.axis_x))
                                .cursor_pointer()
                                .child(glyph(theme, "trash"))
                                .on_click(cx.listener(move |this, _, _window, cx| {
                                    this.document.update(cx, |document, cx| {
                                        if document.remove_variable(index) {
                                            cx.notify();
                                        }
                                    });
                                })),
                        ),
                )
                .when_some(editor, |row, editor| row.child(editor))
                .when_some(error, |row, error| {
                    row.child(
                        div()
                            .text_size(px(theme.text_sm))
                            .text_color(theme.axis_x)
                            .child(error),
                    )
                }),
        );
    }
    ui::surface(theme)
        .w(px(tool_strip::LEFT_WIDTH))
        .flex()
        .flex_col()
        .pb(theme.space(1.5))
        .child(
            div()
                .flex()
                .items_center()
                .px(theme.space(3.0))
                .py(theme.space(2.0))
                .text_size(px(theme.text_sm))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_faint)
                .child("变量")
                .child(
                    div()
                        .id("add-variable")
                        .ml_auto()
                        .px(theme.space(1.5))
                        .rounded(px(4.0))
                        .bg(theme.accent_wash)
                        .text_color(theme.accent)
                        .cursor_pointer()
                        .child("+")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.add_variable_from_panel(window, cx);
                        })),
                ),
        )
        .child(list)
}

fn items_panel(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let document = app.document.read(cx);
    let mut rows: Vec<_> = document
        .bodies
        .iter()
        .map(|body| ItemRow {
            kind: ItemKind::Body(body.id),
            name: if app.renaming_body == Some(body.id) {
                app.rename_buffer.clone().into()
            } else {
                body.name.clone().into()
            },
            color: Rgba {
                r: body.material.base_color[0],
                g: body.material.base_color[1],
                b: body.material.base_color[2],
                a: 1.0,
            }
            .into(),
            visible: body.visible,
            selected: document
                .selection
                .items
                .iter()
                .any(|item| item.body_id() == Some(body.id)),
            editing: app.renaming_body == Some(body.id),
            grounded: document.grounded.contains(&body.id),
        })
        .collect();
    rows.extend(document.sketches.iter().enumerate().map(|(index, sketch)| {
        ItemRow {
            kind: ItemKind::Sketch(sketch.id),
            name: format!("Sketch {}", index + 1).into(),
            color: theme.axis_y,
            visible: sketch.visible,
            selected: document.active_sketch == Some(sketch.id)
                || document
                    .selection
                    .items
                    .iter()
                    .any(|item| matches!(item, SelItem::Profile(id, _) if *id == sketch.id)),
            editing: false,
            grounded: false,
        }
    }));
    rows.extend(document.construction_planes.iter().map(|plane| ItemRow {
        kind: ItemKind::Plane(plane.id),
        name: if app.renaming_plane == Some(plane.id) {
            app.rename_buffer.clone().into()
        } else {
            plane.name.clone().into()
        },
        color: theme.accent,
        visible: plane.visible,
        selected: document.selection.items.contains(&SelItem::Plane(plane.id)),
        editing: app.renaming_plane == Some(plane.id),
        grounded: false,
    }));
    rows.extend(document.construction_axes.iter().map(|axis| ItemRow {
        kind: ItemKind::Axis(axis.id),
        name: axis.name.clone().into(),
        color: theme.accent,
        visible: axis.visible,
        selected: document.selection.items.contains(&SelItem::Axis(axis.id)),
        editing: false,
        grounded: false,
    }));
    rows.extend(document.construction_points.iter().map(|point| ItemRow {
        kind: ItemKind::Point(point.id),
        name: point.name.clone().into(),
        color: theme.axis_x,
        visible: point.visible,
        selected: document.selection.items.contains(&SelItem::Point(point.id)),
        editing: false,
        grounded: false,
    }));
    rows.extend(document.reference_images.iter().map(|image| ItemRow {
        kind: ItemKind::ReferenceImage(image.id),
        name: if app.renaming_reference_image == Some(image.id) {
            app.rename_buffer.clone().into()
        } else {
            image.name.clone().into()
        },
        color: theme.text_muted,
        visible: image.visible,
        selected: false,
        editing: app.renaming_reference_image == Some(image.id),
        grounded: false,
    }));
    let joints = document.joints.clone();
    let over_constrained = document.over_constrained;
    let _ = document;
    // Fixed cap keeps the card clear of the bottom-left modes group; longer
    // lists scroll within.
    let mut list = div()
        .id("items-rows")
        .flex()
        .flex_col()
        .gap(px(1.0))
        .px(theme.space(1.0))
        .max_h(px(190.0))
        .overflow_y_scroll();
    for (index, row) in rows.iter().enumerate() {
        list = list.child(item_row(theme, index, row, cx));
    }
    if !joints.is_empty() {
        list = list.child(
            div()
                .mt(theme.space(1.5))
                .px(theme.space(2.0))
                .text_size(px(theme.text_sm))
                .text_color(if over_constrained {
                    theme.axis_x
                } else {
                    theme.text_faint
                })
                .child(if over_constrained {
                    "关节 · 过约束"
                } else {
                    "关节"
                }),
        );
    }
    for (index, joint) in joints.into_iter().enumerate() {
        let id = joint.id;
        let (a, b) = (joint.a.0, joint.b.0);
        let kind = match joint.kind {
            JointKind::Fixed => "固定",
            JointKind::Revolute => "旋转",
            JointKind::Slider => "滑动",
            JointKind::Cylindrical => "圆柱",
            JointKind::Ball => "球",
        };
        let value = match joint.kind {
            JointKind::Revolute => format!("{:.1}°", joint.value.to_degrees()),
            JointKind::Slider | JointKind::Cylindrical => format!("{:.2} mm", joint.value),
            JointKind::Fixed | JointKind::Ball => "—".to_owned(),
        };
        let editor = app
            .joint_editor
            .as_ref()
            .filter(|(editing, _)| *editing == id)
            .map(|(_, input)| input.clone());
        list = list.child(
            div()
                .id(("joint-row", index))
                .flex()
                .items_center()
                .gap(theme.space(1.0))
                .px(theme.space(2.0))
                .py(theme.space(1.0))
                .rounded(px(5.0))
                .hover(|row| row.bg(theme.hover))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.document.update(cx, |document, cx| {
                        document.selection.items = vec![SelItem::Body(a), SelItem::Body(b)];
                        cx.notify();
                    });
                }))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(theme.text_sm))
                        .child(format!("{} · {kind}", joint.name)),
                )
                .child(
                    div()
                        .id(("joint-value", index))
                        .text_size(px(theme.text_sm))
                        .text_color(theme.accent)
                        .when_some(editor, |field, input| field.w(px(74.0)).child(input))
                        .when(
                            app.joint_editor
                                .as_ref()
                                .is_none_or(|(editing, _)| *editing != id),
                            |field| field.child(value),
                        )
                        .on_click(cx.listener(
                            move |this, event: &gpui::ClickEvent, window, cx| {
                                cx.stop_propagation();
                                if event.click_count() >= 2 {
                                    this.begin_joint_edit(id, window, cx);
                                }
                            },
                        )),
                )
                .child(
                    div()
                        .id(("delete-joint", index))
                        .px(px(3.0))
                        .text_color(theme.text_faint)
                        .child("×")
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.document.update(cx, |document, cx| {
                                document.delete_joint(id);
                                cx.notify();
                            });
                        })),
                ),
        );
    }
    ui::surface(theme)
        .w(px(tool_strip::LEFT_WIDTH))
        .flex()
        .flex_col()
        .pb(theme.space(1.5))
        .child(panel_header(theme, "项目"))
        .child(list)
}

fn history_panel(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let rows = app.document.read(cx).history.clone();
    let mut list = div().flex().flex_col().gap(px(1.0)).px(theme.space(1.0));
    for (index, step) in rows.iter().enumerate() {
        list = list.child(history_row(app, index, step, cx));
    }
    ui::surface(theme)
        .w(px(tool_strip::LEFT_WIDTH))
        .flex()
        .flex_col()
        .pb(theme.space(1.5))
        .child(panel_header(theme, "历史记录"))
        .child(list)
}

fn panel_header(theme: &Theme, title: &'static str) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .px(theme.space(3.0))
        .py(theme.space(2.0))
        .text_size(px(theme.text_sm))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_faint)
        .child(title.to_uppercase())
}

/// Renders one Items row from a plain [`ItemRow`].
pub fn item_row(
    theme: &Theme,
    index: usize,
    row: &ItemRow,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let name = row.name.clone();
    let visible = row.visible;
    let selected = row.selected;
    let kind = row.kind;
    let grounded = row.grounded;
    let group = format!("item-row-{index}");
    div()
        .id(("item", index))
        .group(group.clone())
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.space(2.0))
        .px(theme.space(2.0))
        .py(theme.space(1.5))
        .rounded(px(theme.radius_control))
        .when(selected, |row| row.bg(theme.accent_wash))
        .hover(|s| s.bg(theme.hover))
        .cursor_pointer()
        .on_click(cx.listener(
            move |this, event: &gpui::ClickEvent, window, cx| match kind {
                ItemKind::Body(id) if event.click_count() >= 2 => {
                    this.begin_rename(id, window, cx);
                }
                ItemKind::Plane(id) if event.click_count() >= 2 => {
                    this.begin_plane_rename(id, window, cx);
                }
                ItemKind::ReferenceImage(id) if event.click_count() >= 2 => {
                    this.begin_reference_image_rename(id, window, cx);
                }
                ItemKind::Body(id) => {
                    this.document.update(cx, |document, cx| {
                        document.selection.apply(SelItem::Body(id), false);
                        cx.notify();
                    });
                    cx.notify();
                }
                ItemKind::Sketch(id) => {
                    this.document.update(cx, |document, cx| {
                        document.active_sketch = Some(id);
                        document.selection.clear();
                        cx.notify();
                    });
                    cx.notify();
                }
                ItemKind::Plane(id) => {
                    this.document.update(cx, |document, cx| {
                        document.selection.apply(SelItem::Plane(id), false);
                        cx.notify();
                    });
                    cx.notify();
                }
                ItemKind::Axis(id) => {
                    this.document.update(cx, |document, cx| {
                        document.selection.apply(SelItem::Axis(id), false);
                        cx.notify();
                    });
                    cx.notify();
                }
                ItemKind::Point(id) => {
                    this.document.update(cx, |document, cx| {
                        document.selection.apply(SelItem::Point(id), false);
                        cx.notify();
                    });
                    cx.notify();
                }
                ItemKind::ReferenceImage(_) => {}
            },
        ))
        .child(
            div()
                .size(px(10.0))
                .rounded_full()
                .bg(row.color)
                .border_1()
                .border_color(theme.border_strong),
        )
        .child(
            div()
                .flex_1()
                .when(row.editing, |name| {
                    name.border_1().border_color(theme.accent).px(px(3.0))
                })
                .text_size(px(theme.text_md))
                .text_color(if visible {
                    theme.text
                } else {
                    theme.text_faint
                })
                .child(name),
        )
        .when(matches!(kind, ItemKind::Body(_)), |row| {
            row.child(
                div()
                    .id(("grounded", index))
                    .size(px(22.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.0))
                    .text_color(if grounded {
                        theme.accent
                    } else {
                        theme.text_faint
                    })
                    .cursor_pointer()
                    .child("⚓")
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        cx.stop_propagation();
                        if let ItemKind::Body(id) = kind {
                            this.document.update(cx, |document, cx| {
                                document.toggle_grounded(id);
                                cx.notify();
                            });
                        }
                    })),
            )
        })
        .child(
            div()
                .id(("visibility", index))
                .size(px(24.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.0))
                .text_color(if visible {
                    theme.text_muted
                } else {
                    theme.text_faint
                })
                .hover(|s| s.bg(theme.active).text_color(theme.text))
                .cursor_pointer()
                .child(glyph(theme, if visible { "eye" } else { "eye-off" }))
                .on_click(cx.listener(move |this, _, _window, cx| {
                    cx.stop_propagation();
                    this.document.update(cx, |document, cx| {
                        match kind {
                            ItemKind::Body(id) => document.set_visible(id, !visible),
                            ItemKind::Sketch(id) => document.set_sketch_visible(id, !visible),
                            ItemKind::Plane(id) => {
                                document.set_construction_plane_visible(id, !visible)
                            }
                            ItemKind::Axis(id) => {
                                document.set_construction_axis_visible(id, !visible)
                            }
                            ItemKind::Point(id) => {
                                document.set_construction_point_visible(id, !visible)
                            }
                            ItemKind::ReferenceImage(id) => {
                                document.set_reference_image_visible(id, !visible)
                            }
                        }
                        cx.notify();
                    });
                    cx.notify();
                })),
        )
        .when(
            matches!(
                kind,
                ItemKind::Body(_)
                    | ItemKind::Plane(_)
                    | ItemKind::Axis(_)
                    | ItemKind::Point(_)
                    | ItemKind::ReferenceImage(_)
            ),
            |row| {
                row.child(
                    div()
                        .id(("delete", index))
                        .invisible()
                        .group_hover(group, |button| button.visible())
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .text_color(theme.text_muted)
                        .hover(|style| style.bg(theme.active).text_color(theme.axis_x))
                        .cursor_pointer()
                        .child(glyph(theme, "trash"))
                        .on_click(cx.listener(move |this, _, _window, cx| {
                            cx.stop_propagation();
                            this.document.update(cx, |document, cx| {
                                match kind {
                                    ItemKind::Body(id) => document.remove_bodies(&[id]),
                                    ItemKind::Plane(id) => {
                                        document.remove_construction_plane(id);
                                    }
                                    ItemKind::Axis(id) => {
                                        document.remove_construction_axis(id);
                                    }
                                    ItemKind::Point(id) => {
                                        document.remove_construction_point(id);
                                    }
                                    ItemKind::ReferenceImage(id) => {
                                        document.remove_reference_image(id);
                                    }
                                    ItemKind::Sketch(_) => {}
                                }
                                cx.notify();
                            });
                            cx.notify();
                        })),
                )
            },
        )
}

/// Renders one editable History row.
pub fn history_row(
    app: &Free3dApp,
    index: usize,
    step: &HistoryStep,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    let suppressed = step.suppressed;
    let editable = step.op.numeric_value().is_some();
    let failed = app
        .replay_error
        .as_ref()
        .is_some_and(|(failed, _)| *failed == index);
    let downstream = app
        .replay_error
        .as_ref()
        .is_some_and(|(failed, _)| index > *failed);
    let group = format!("history-row-{index}");
    let mut row = div()
        .id(("history", index))
        .group(group.clone())
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.space(2.0))
        .px(theme.space(2.0))
        .py(theme.space(1.5))
        .rounded(px(theme.radius_control))
        .when(failed, |row| row.border_1().border_color(theme.axis_x))
        .when(downstream, |row| row.bg(theme.accent_wash))
        .when(suppressed, |row| row.opacity(0.48))
        .hover(|s| s.bg(theme.hover))
        .when(editable, |row| {
            row.cursor_pointer()
                .on_click(cx.listener(move |app, _, window, cx| {
                    app.begin_history_edit(index, HistoryNumericField::Primary, window, cx);
                }))
        })
        .child(
            div()
                .text_color(theme.text_muted)
                .child(glyph(theme, step.op.icon())),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(theme.text_md))
                .text_color(theme.text)
                .child(step.op.label()),
        )
        .child(
            div()
                .text_size(px(theme.text_sm))
                .text_color(theme.text_faint)
                .child(step.op.summary()),
        );
    if matches!(step.op, HistoryOp::LinearPattern { .. }) {
        row = row.child(
            div()
                .id(("history-count", index))
                .px(px(3.0))
                .rounded(px(4.0))
                .text_size(px(theme.text_sm))
                .text_color(theme.text_muted)
                .hover(|item| item.bg(theme.active).text_color(theme.text))
                .child("#")
                .on_click(cx.listener(move |app, _, window, cx| {
                    cx.stop_propagation();
                    app.begin_history_edit(index, HistoryNumericField::Count, window, cx);
                })),
        );
    }
    row = row
        .child(
            div()
                .id(("history-suppress", index))
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .text_color(theme.text_muted)
                .hover(|item| item.bg(theme.active).text_color(theme.text))
                .child(glyph(theme, if suppressed { "eye-off" } else { "eye" }))
                .on_click(cx.listener(move |app, _, _window, cx| {
                    cx.stop_propagation();
                    app.toggle_history_step(index, cx);
                })),
        )
        .child(
            div()
                .id(("history-delete", index))
                .invisible()
                .group_hover(group, |button| button.visible())
                .size(px(22.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.0))
                .text_color(theme.text_muted)
                .hover(|item| item.bg(theme.active).text_color(theme.axis_x))
                .child(glyph(theme, "trash"))
                .on_click(cx.listener(move |app, _, _window, cx| {
                    cx.stop_propagation();
                    app.delete_history_step(index, cx);
                })),
        );
    let mut column = div().flex().flex_col().child(row);
    if let Some((editor_index, _, input)) = &app.history_editor
        && *editor_index == index
    {
        column = column.child(div().pl(px(34.0)).py(px(3.0)).child(input.clone()));
    }
    if let Some((error_index, message)) = &app.replay_error
        && *error_index == index
    {
        column = column.child(
            div()
                .pl(px(34.0))
                .pb(px(4.0))
                .text_size(px(theme.text_sm))
                .text_color(theme.axis_x)
                .child(message.clone()),
        );
    }
    column
}
