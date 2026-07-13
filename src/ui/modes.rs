//! Bottom-left interaction mode strip: compact icon toggles for Section,
//! Isolate, Measure, and Exploded, with the Exploded slider revealed only
//! while that mode is active.

use gpui::{Context, MouseButton, MouseMoveEvent, div, prelude::*, px};

use crate::{
    app::DuctileApp,
    commands::{AppCommand, ModeChip},
    ui::{self, glyph, tip},
};

/// Builds the compact mode strip pinned to the bottom of the left rail.
pub fn render(app: &DuctileApp, cx: &mut Context<DuctileApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut group = ui::surface(theme)
        .mt_auto()
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

    if app.mode_active(ModeChip::Exploded) {
        group = group.child(exploded_row(app, cx));
    }

    let mut strip = div().flex().flex_row().gap(px(1.0));
    for chip in ModeChip::ALL {
        strip = strip.child(mode_button(app, chip, cx));
    }
    group.child(strip)
}

/// One icon toggle in the strip, labelled via its hover tooltip.
fn mode_button(app: &DuctileApp, chip: ModeChip, cx: &mut Context<DuctileApp>) -> impl IntoElement {
    let theme = &app.theme;
    let active = app.mode_active(chip);
    let enabled = app.mode_enabled(chip, cx);
    let fg = if active {
        theme.accent
    } else {
        theme.text_muted
    };

    div()
        .id(("mode", chip as usize))
        .size(px(30.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme.radius_control))
        .when(active, |button| button.bg(theme.accent_wash))
        .when(enabled, |button| {
            button
                .hover(|s| s.bg(theme.hover))
                .active(|s| s.bg(theme.active))
                .cursor_pointer()
        })
        .when(!enabled, |button| button.opacity(0.42))
        .child(glyph(theme, chip.icon()).text_color(fg))
        .tooltip(tip(theme, chip.label(), None))
        .on_click(cx.listener(move |this, _, window, cx| {
            if this.mode_enabled(chip, cx) {
                this.dispatch(AppCommand::ToggleMode(chip), window, cx);
            }
        }))
}

/// The Exploded slider row shown above the strip while that mode is active.
fn exploded_row(app: &DuctileApp, cx: &mut Context<DuctileApp>) -> impl IntoElement {
    let theme = &app.theme;
    let fill = app.exploded_factor.clamp(0.0, 1.0);
    div()
        .px(theme.space(1.0))
        .py(theme.space(1.0))
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
        )
}
