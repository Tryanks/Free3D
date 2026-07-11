//! Top-right floating clusters below the viewport-rendered orientation cube.
//!
//! An upper cluster carries the snap magnet, the grid-pitch indicator and the
//! views popover trigger; a lower cluster carries the display-mode, screenshot
//! and history controls.

use gpui::{
    Anchor, AnchoredPositionMode, Context, ElementId, FontWeight, MouseButton, Stateful, anchored,
    deferred, div, point, prelude::*, px,
};

use crate::{
    app::{Free3dApp, Space},
    commands::{AppCommand, StandardView},
    theme::Theme,
    ui::{self, glyph},
    viewport::{AnalysisMode, DisplayMode},
};

/// Width of the FOV scrub track, in logical pixels.
const FOV_TRACK: f32 = 176.0;
/// Width of the labelled lower cluster.
const CLUSTER_WIDTH: f32 = 172.0;

/// Builds the top-right view clusters.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    // Keep the controls clear of the 96 px cube at a 24 px viewport inset.
    div()
        .absolute()
        .top(px(132.0))
        .right(px(24.0))
        .flex()
        .flex_col()
        .items_end()
        .gap(app.theme.space(2.0))
        .when(app.space == Space::Modeling, |cluster| {
            cluster.child(upper_cluster(app, cx))
        })
        .child(if app.space == Space::Drawing {
            screenshot_cluster(app, cx).into_any_element()
        } else {
            lower_cluster(app, cx).into_any_element()
        })
}

fn screenshot_cluster(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    ui::surface(theme).p(theme.space(1.0)).child(
        info_row(
            theme,
            "drawing-screenshot",
            "camera",
            "截图",
            None,
            None,
            false,
        )
        .on_click(
            cx.listener(|this, _, window, cx| this.dispatch(AppCommand::Screenshot, window, cx)),
        ),
    )
}

/// The snap / grid-pitch / views cluster and its optional popover.
fn upper_cluster(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    div().relative().child(
        ui::surface(theme)
            .flex()
            .flex_col()
            .items_center()
            .gap(theme.space(0.5))
            .p(theme.space(1.0))
            .child(
                ui::icon_button(theme, "snap", "magnet", app.snap_enabled)
                    .tooltip(ui::tip(theme, "捕捉", None))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.dispatch(AppCommand::ToggleSnap, window, cx)
                    })),
            )
            .child(grid_indicator(app, cx))
            .child(
                ui::icon_button(theme, "views", "views", app.show_views)
                    .tooltip(ui::tip(theme, "视图", None))
                    .on_click(cx.listener(|this, _, _window, cx| {
                        this.show_views = !this.show_views;
                        cx.notify();
                    })),
            )
            .when(app.show_views, |pill| pill.child(popover(app, cx))),
    )
}

/// The grid-lock indicator: a two-line micro-label showing the grid pitch.
fn grid_indicator(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let on = app.grid_visible;
    let fg = if on { theme.accent } else { theme.text_muted };
    let pitch = app
        .units
        .display_value(f64::from(app.viewport.read(cx).grid_pitch()));
    let label = if pitch >= 1.0 {
        format!("{}", pitch.round() as i64)
    } else {
        format!("{pitch:.1}")
    };
    div()
        .id("grid-pitch")
        .w(px(theme.control))
        .h(px(theme.control))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .rounded(px(theme.radius_control))
        .when(on, |b| b.bg(theme.accent_wash))
        .hover(|s| s.bg(theme.hover))
        .active(|s| s.bg(theme.active))
        .cursor_pointer()
        .tooltip(ui::tip(theme, "网格间距", None))
        .child(
            div()
                .text_size(px(theme.text_md))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg)
                .child(label),
        )
        .child(
            div()
                .text_size(px(9.0))
                .text_color(theme.text_faint)
                .child(app.units.symbol()),
        )
        .on_click(
            cx.listener(|this, _, window, cx| this.dispatch(AppCommand::ToggleGrid, window, cx)),
        )
}

/// The display-mode / screenshot / history cluster.
fn lower_cluster(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    ui::surface(theme)
        .w(px(CLUSTER_WIDTH))
        .flex()
        .flex_col()
        .gap(px(1.0))
        .p(theme.space(1.0))
        .child(
            info_row(
                theme,
                "display-mode",
                "display",
                "显示模式",
                Some(app.display_mode.label()),
                None,
                app.display_mode != DisplayMode::Shaded,
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.dispatch(AppCommand::ToggleWireframe, window, cx)
            })),
        )
        .child(
            info_row(theme, "screenshot", "camera", "截图", None, None, false).on_click(
                cx.listener(|this, _, window, cx| {
                    this.dispatch(AppCommand::Screenshot, window, cx)
                }),
            ),
        )
        .when(app.space == Space::Modeling, |cluster| {
            cluster.child(
                info_row(
                    theme,
                    "history-toggle",
                    "history",
                    "历史记录",
                    None,
                    Some("⌥⌘P"),
                    app.show_history,
                )
                .on_click(cx.listener(|this, _, window, cx| {
                    this.dispatch(AppCommand::ToggleHistoryPanel, window, cx)
                })),
            )
        })
}

/// A right-cluster row: label (with optional state line) on the left, an
/// optional shortcut chip, and the icon on the right near the screen edge.
fn info_row(
    theme: &Theme,
    id: impl Into<ElementId>,
    icon: &str,
    label: &'static str,
    sub: Option<&'static str>,
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
        .flex_col()
        .gap(px(1.0))
        .px(theme.space(1.5))
        .py(theme.space(1.0))
        .rounded(px(theme.radius_control))
        .when(active, |row| row.bg(theme.accent_wash))
        .hover(|s| s.bg(theme.hover))
        .active(|s| s.bg(theme.active))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.space(1.5))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(theme.text_md))
                        .text_color(label_color)
                        .child(label),
                )
                .when_some(shortcut, |row, shortcut| {
                    row.child(
                        div()
                            .px(theme.space(1.0))
                            .py(px(1.0))
                            .rounded(px(4.0))
                            .bg(theme.well)
                            .border_1()
                            .border_color(theme.border)
                            .text_color(theme.text_muted)
                            .text_size(px(theme.text_sm))
                            .font_weight(FontWeight::MEDIUM)
                            .child(shortcut),
                    )
                })
                .child(
                    div()
                        .w(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(glyph(theme, icon).text_color(fg)),
                ),
        )
        .when_some(sub, |row, sub| {
            row.child(
                div()
                    .text_size(px(theme.text_sm))
                    .text_color(theme.text_faint)
                    .child(sub),
            )
        })
}

/// The floating views / appearance popover.
fn popover(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let fov = app.fov_degrees;
    let fill = (fov / 90.0).clamp(0.0, 1.0);

    let panel = ui::surface_elevated(theme)
        .w(px(224.0))
        .flex()
        .flex_col()
        .gap(theme.space(2.0))
        .p(theme.space(2.5))
        // Continue an FOV scrub anywhere over the popover, and end it on release.
        .on_mouse_move(
            cx.listener(|this, event: &gpui::MouseMoveEvent, window, cx| {
                this.drag_fov(f32::from(event.position.x), FOV_TRACK, window, cx);
            }),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _, _window, _cx| this.end_fov_drag()),
        )
        .on_mouse_up_out(
            MouseButton::Left,
            cx.listener(|this, _, _window, _cx| this.end_fov_drag()),
        )
        .on_mouse_down_out(cx.listener(|this, _, _window, cx| {
            this.show_views = false;
            cx.notify();
        }))
        .child(section_label(theme, "标准视图"))
        .child(standard_views(app, cx))
        .child(ui::divider(theme, false))
        .child(section_label(theme, "视图"))
        .child(saved_views(app, cx))
        .child(ui::divider(theme, false))
        .child(fov_row(theme, fov, fill, cx))
        .child(ui::divider(theme, false))
        .child(section_label(theme, "表面分析"))
        .child(analysis_row(app, AnalysisMode::Zebra, "斑马纹", cx))
        .child(analysis_row(app, AnalysisMode::Curvature, "曲率", cx))
        .child(ui::divider(theme, false))
        .child(grid_row(app, cx));

    deferred(
        anchored()
            .anchor(Anchor::TopRight)
            .position(point(px(0.0), px(0.0)))
            .position_mode(AnchoredPositionMode::Local)
            .snap_to_window_with_margin(px(8.0))
            .child(panel),
    )
    .priority(1)
}

fn analysis_row(
    app: &Free3dApp,
    mode: AnalysisMode,
    label: &'static str,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    let on = app.analysis == mode;
    div()
        .id(("surface-analysis", mode as usize))
        .flex()
        .items_center()
        .justify_between()
        .text_size(px(theme.text_md))
        .text_color(if on { theme.accent } else { theme.text_muted })
        .cursor_pointer()
        .child(label)
        .child(toggle_switch(theme, on))
        .on_click(cx.listener(move |this, _, _window, cx| this.toggle_analysis(mode, cx)))
}

fn saved_views(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut slots = div().flex().flex_row().flex_wrap().gap(theme.space(1.0));
    for index in 0..crate::saved_views::SavedViews::LEN {
        let filled = app.saved_views.get(index).is_some();
        let label = if filled {
            format!("视图 {}", index + 1)
        } else {
            "+ 保存视图".to_owned()
        };
        slots = slots.child(
            div()
                .id(("saved-view", index))
                .w(px(84.0))
                .h(px(30.0))
                .px(theme.space(1.5))
                .flex()
                .items_center()
                .gap(theme.space(1.0))
                .rounded(px(theme.radius_control))
                .bg(theme.well)
                .border_1()
                .border_color(theme.border)
                .text_size(px(theme.text_sm))
                .text_color(if filled { theme.text } else { theme.text_muted })
                .hover(|row| row.bg(theme.hover).border_color(theme.border_strong))
                .cursor_pointer()
                .child(div().flex_1().child(label))
                .when(filled, |row| {
                    row.child(
                        div()
                            .id(("clear-saved-view", index))
                            .size(px(18.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .text_color(theme.text_faint)
                            .hover(|button| button.bg(theme.active).text_color(theme.text))
                            .child("×")
                            .on_click(cx.listener(move |this, _, _window, cx| {
                                cx.stop_propagation();
                                this.clear_saved_view(index, cx);
                            })),
                    )
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.use_saved_view(index, window, cx);
                })),
        );
    }
    slots
}

fn section_label(theme: &Theme, text: &'static str) -> impl IntoElement {
    div()
        .text_size(px(theme.text_sm))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_faint)
        .child(text)
}

fn standard_views(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut grid = div().flex().flex_row().flex_wrap().gap(theme.space(1.0));
    for view in StandardView::ALL {
        grid = grid.child(view_button(app, view, cx));
    }
    grid
}

fn view_button(
    app: &Free3dApp,
    view: StandardView,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    let accent = matches!(view, StandardView::Iso);
    div()
        .id(("view", view as usize))
        .flex()
        .items_center()
        .justify_center()
        .h(px(28.0))
        .px(theme.space(2.5))
        .rounded(px(theme.radius_control))
        .bg(theme.well)
        .border_1()
        .border_color(theme.border)
        .text_size(px(theme.text_sm))
        .text_color(if accent { theme.accent } else { theme.text })
        .hover(|s| s.bg(theme.hover).border_color(theme.border_strong))
        .active(|s| s.bg(theme.active))
        .cursor_pointer()
        .child(view.label())
        .on_click(cx.listener(move |this, _, window, cx| {
            this.dispatch(AppCommand::StandardView(view), window, cx)
        }))
}

fn fov_row(theme: &Theme, fov: f32, fill: f32, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(theme.space(1.5))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .text_size(px(theme.text_md))
                        .text_color(theme.text_muted)
                        .child("视场角"),
                )
                .child(
                    div()
                        .text_size(px(theme.text_md))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text)
                        .child(format!("{}\u{00b0}", fov.round() as i32)),
                ),
        )
        .child(
            // Scrub track: press and drag horizontally to change the FOV.
            div()
                .id("fov-track")
                .w(px(FOV_TRACK))
                .h(px(18.0))
                .flex()
                .items_center()
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, _window, _cx| {
                        this.begin_fov_drag(f32::from(event.position.x));
                    }),
                )
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(4.0))
                        .rounded_full()
                        .bg(theme.well)
                        .child(
                            div()
                                .absolute()
                                .left(px(0.0))
                                .top(px(0.0))
                                .h(px(4.0))
                                .w(px(FOV_TRACK * fill))
                                .rounded_full()
                                .bg(theme.accent),
                        )
                        .child(
                            div()
                                .absolute()
                                .top(px(-5.0))
                                .left(px((FOV_TRACK * fill - 7.0).max(0.0)))
                                .size(px(14.0))
                                .rounded_full()
                                .bg(theme.text)
                                .border_2()
                                .border_color(theme.accent),
                        ),
                ),
        )
}

fn grid_row(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let on = app.grid_visible;
    div()
        .id("grid-row")
        .flex()
        .flex_row()
        .justify_between()
        .items_center()
        .cursor_pointer()
        .child(
            div()
                .text_size(px(theme.text_md))
                .text_color(theme.text_muted)
                .child("网格平面"),
        )
        .child(toggle_switch(theme, on))
        .on_click(
            cx.listener(|this, _, window, cx| this.dispatch(AppCommand::ToggleGrid, window, cx)),
        )
}

/// A small on/off switch reflecting `on`.
fn toggle_switch(theme: &Theme, on: bool) -> impl IntoElement {
    let mut track = div()
        .w(px(34.0))
        .h(px(20.0))
        .flex()
        .items_center()
        .p(px(2.0))
        .rounded_full();
    track = if on {
        track.bg(theme.accent).justify_end()
    } else {
        track
            .bg(theme.well)
            .border_1()
            .border_color(theme.border_strong)
            .justify_start()
    };
    track.child(div().size(px(14.0)).rounded_full().bg(if on {
        theme.on_accent
    } else {
        theme.text_muted
    }))
}
