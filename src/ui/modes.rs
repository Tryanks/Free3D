//! Bottom-left interaction mode group: Section (with a state line), Isolate,
//! and Measure, stacked vertically as icon + label rows.

use gpui::{Context, MouseButton, MouseMoveEvent, div, prelude::*, px};

use crate::{
    app::Free3dApp,
    commands::{AppCommand, ModeChip},
    ui::{self, glyph},
};

/// Builds the vertical mode group anchored to the bottom-left corner.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut group = ui::surface(theme)
        .absolute()
        .left(px(super::tool_strip::LEFT_INSET))
        .bottom(theme.space(3.0))
        .w(px(super::tool_strip::LEFT_WIDTH))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .p(theme.space(1.0))
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
            if this.exploded_dragging {
                this.drag_exploded(f32::from(event.position.x), 120.0, window, cx);
            }
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _, _| this.end_exploded_drag()),
        );

    for chip in ModeChip::ALL {
        group = group.child(mode_chip(app, chip, cx));
    }
    group
}

fn mode_chip(app: &Free3dApp, chip: ModeChip, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let active = app.mode_active(chip);
    let enabled = app.mode_enabled(chip, cx);
    let fg = if active {
        theme.accent
    } else {
        theme.text_muted
    };
    // Icon column width plus the row gap, so the Section state line aligns
    // under the label rather than the icon.
    let indent = px(theme.icon + f32::from(theme.space(1.5)));

    div()
        .id(("mode", chip as usize))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .px(theme.space(2.0))
        .py(theme.space(1.5))
        .rounded(px(theme.radius_control))
        .when(active, |c| c.bg(theme.accent_wash))
        .when(enabled, |chip| {
            chip.hover(|s| s.bg(theme.hover))
                .active(|s| s.bg(theme.active))
                .cursor_pointer()
        })
        .when(!enabled, |chip| chip.opacity(0.42))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.space(1.5))
                .child(glyph(theme, chip.icon()).text_color(fg))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(theme.text_md))
                        .text_color(if active { theme.accent } else { theme.text })
                        .child(chip.label()),
                ),
        )
        .when(chip == ModeChip::Section, |entry| {
            entry.child(
                div()
                    .pl(indent)
                    .text_size(px(theme.text_sm))
                    .text_color(theme.text_faint)
                    .child(chip.state_label(active)),
            )
        })
        .when(chip == ModeChip::Exploded, |entry| {
            let fill = app.exploded_factor.clamp(0.0, 1.0);
            entry.child(
                div()
                    .pl(indent)
                    .flex()
                    .items_center()
                    .gap(theme.space(1.0))
                    .child(
                        div()
                            .id("exploded-track")
                            .relative()
                            .w(px(120.0))
                            .h(px(14.0))
                            .flex()
                            .items_center()
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &gpui::MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    this.begin_exploded_drag(f32::from(event.position.x));
                                }),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(4.0))
                                    .rounded_full()
                                    .bg(theme.well)
                                    .child(
                                        div()
                                            .w(px(120.0 * fill))
                                            .h(px(4.0))
                                            .rounded_full()
                                            .bg(theme.accent),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .w(px(28.0))
                            .text_size(px(theme.text_sm))
                            .text_color(theme.text_faint)
                            .child(format!("{:.2}", app.exploded_factor)),
                    ),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            if this.mode_enabled(chip, cx) {
                this.dispatch(AppCommand::ToggleMode(chip), window, cx);
            }
        }))
}
