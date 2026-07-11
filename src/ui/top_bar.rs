//! Floating top bar: a compact project pill on the left, share / help on the right.

use gpui::{
    Anchor, AnchoredPositionMode, Context, FontWeight, anchored, deferred, div, point, prelude::*,
    px,
};

use crate::{
    app::Free3dApp,
    commands::AppCommand,
    i18n::{LangChoice, ZH_ENDONYM},
    nav::NavPreset,
    ui::{self, glyph},
    units::Units,
};

/// Builds the floating top bar as two anchored pills over the viewport.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    div()
        .absolute()
        .top(theme.space(3.0))
        .left(theme.space(3.0))
        .right(theme.space(3.0))
        .flex()
        .flex_row()
        .justify_between()
        .items_start()
        .child(left_pill(app, cx))
        .child(right_pill(app, cx))
}

fn left_pill(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    ui::surface(theme)
        .h(px(theme.control + 6.0))
        .flex()
        .flex_row()
        .items_center()
        .gap(theme.space(0.5))
        .px(theme.space(1.0))
        .child(
            ui::icon_button(theme, "home", "home", false)
                .tooltip(ui::tip(theme, crate::i18n::t("Home"), None))
                .on_click(cx.listener(|this, _, window, cx| this.go_home(window, cx))),
        )
        .child(
            ui::icon_button(theme, "sync", "sync", false)
                .tooltip(ui::tip(theme, crate::i18n::t("Sync"), None))
                .on_click(cx.listener(|_, _, _window, _cx| {})),
        )
        .child(ui::divider(theme, true))
        .child(
            div()
                .id("project")
                .px(theme.space(2.0))
                .py(theme.space(1.0))
                .rounded(px(theme.radius_control))
                .text_color(theme.text)
                .text_size(px(theme.text_md))
                .font_weight(FontWeight::MEDIUM)
                .hover(|s| s.bg(theme.hover))
                .cursor_pointer()
                .child(app.project_label(cx)),
        )
}

fn right_pill(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    div()
        .relative()
        .child(
            ui::surface(theme)
                .h(px(theme.control + 6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(theme.space(0.5))
                .px(theme.space(1.0))
                .child(
                    div()
                        .id("share")
                        .h(px(theme.control - 4.0))
                        .px(theme.space(2.5))
                        .flex()
                        .items_center()
                        .gap(theme.space(1.0))
                        .rounded(px(theme.radius_control))
                        .text_color(theme.text)
                        .text_size(px(theme.text_md))
                        .font_weight(FontWeight::MEDIUM)
                        .hover(|s| s.bg(theme.hover))
                        .cursor_pointer()
                        .child(glyph(theme, "share").size(px(theme.icon - 3.0)))
                        .child(crate::i18n::t("Share"))
                        .on_click(cx.listener(|_, _, _window, _cx| {})),
                )
                .child(ui::divider(theme, true))
                .child(
                    ui::icon_button(theme, "settings", "settings", app.show_settings)
                        .tooltip(ui::tip(theme, crate::i18n::t("Settings"), None))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.dispatch(AppCommand::OpenSettings, window, cx)
                        })),
                )
                .child(
                    ui::icon_button(theme, "help", "help", false)
                        .tooltip(ui::tip(theme, crate::i18n::t("Help"), None))
                        .on_click(cx.listener(|_, _, _window, _cx| {})),
                ),
        )
        .when(app.show_settings, |wrapper| {
            wrapper.child(settings_popover(app, cx))
        })
}

fn settings_popover(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut preset_menu = div().flex().flex_col().gap(px(1.0));
    if app.show_nav_presets {
        for preset in NavPreset::ALL {
            preset_menu = preset_menu.child(
                div()
                    .id(("nav-preset", preset as usize))
                    .px(theme.space(2.0))
                    .py(theme.space(1.5))
                    .rounded(px(theme.radius_control))
                    .text_size(px(theme.text_md))
                    .when(app.nav_preset == preset, |row| {
                        row.bg(theme.accent_wash).text_color(theme.accent)
                    })
                    .hover(|row| row.bg(theme.hover))
                    .cursor_pointer()
                    .child(preset.label())
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        this.set_nav_preset(preset, cx);
                    })),
            );
        }
    }

    let panel = ui::surface_elevated(theme)
        .w(px(240.0))
        .p(theme.space(2.5))
        .flex()
        .flex_col()
        .gap(theme.space(2.0))
        .child(settings_label(theme, crate::i18n::t("Theme")))
        .child(
            div()
                .flex()
                .gap(theme.space(1.0))
                .child(theme_button(app, true, cx))
                .child(theme_button(app, false, cx)),
        )
        .child(ui::divider(theme, false))
        .child(settings_label(theme, crate::i18n::t("Language")))
        .child(language_buttons(app, cx))
        .child(ui::divider(theme, false))
        .child(settings_label(theme, crate::i18n::t("Navigation Preset")))
        .child(
            div()
                .id("nav-preset-select")
                .px(theme.space(2.0))
                .h(px(34.0))
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(theme.radius_control))
                .bg(theme.well)
                .border_1()
                .border_color(theme.border_strong)
                .text_size(px(theme.text_md))
                .cursor_pointer()
                .child(app.nav_preset.label())
                .child(if app.show_nav_presets { "▴" } else { "▾" })
                .on_click(cx.listener(|this, _, _window, cx| {
                    this.show_nav_presets = !this.show_nav_presets;
                    cx.notify();
                })),
        )
        .child(preset_menu)
        .child(ui::divider(theme, false))
        .child(settings_label(theme, crate::i18n::t("Units")))
        .child(unit_buttons(app, cx))
        .child(ui::divider(theme, false))
        .child(settings_label(
            theme,
            crate::i18n::t("Autosave interval (seconds, 0 = off)"),
        ))
        .child(autosave_buttons(app, cx))
        .child(ui::divider(theme, false))
        .child(settings_label(theme, crate::i18n::t("Files")))
        .child(
            div()
                .flex()
                .gap(theme.space(1.0))
                .child(file_button(
                    app,
                    crate::i18n::t("Import"),
                    "import",
                    AppCommand::Import,
                    cx,
                ))
                .child(file_button(
                    app,
                    crate::i18n::t("Export"),
                    "export",
                    AppCommand::Export,
                    cx,
                )),
        )
        .when(!app.recent_files.is_empty(), |panel| {
            let mut recent = div().flex().flex_col().gap(px(1.0));
            for (index, path) in app.recent_files.iter().cloned().enumerate() {
                let label = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(crate::i18n::t("Project"))
                    .to_owned();
                recent = recent.child(
                    div()
                        .id(("recent-file", index))
                        .px(theme.space(2.0))
                        .py(theme.space(1.25))
                        .rounded(px(theme.radius_control))
                        .text_size(px(theme.text_sm))
                        .text_color(theme.text)
                        .hover(|row| row.bg(theme.hover))
                        .cursor_pointer()
                        .child(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_recent(path.clone(), window, cx);
                        })),
                );
            }
            panel
                .child(ui::divider(theme, false))
                .child(settings_label(theme, crate::i18n::t("Recent Files")))
                .child(recent)
        });

    deferred(
        anchored()
            .anchor(Anchor::TopRight)
            .position(point(px(0.0), px(theme.control + 12.0)))
            .position_mode(AnchoredPositionMode::Local)
            .snap_to_window_with_margin(px(8.0))
            .child(panel),
    )
    .priority(2)
}

fn language_buttons(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut row = div().flex().gap(theme.space(1.0));
    for (choice, label) in [
        (LangChoice::Auto, crate::i18n::t("Auto")),
        (LangChoice::En, "English"),
        (LangChoice::ZhCn, ZH_ENDONYM),
    ] {
        let selected = app.language == choice;
        row = row.child(
            div()
                .id(("language", choice as usize))
                .px(theme.space(1.5))
                .h(px(30.0))
                .flex()
                .items_center()
                .rounded(px(theme.radius_control))
                .bg(theme.well)
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .text_color(if selected { theme.accent } else { theme.text })
                .text_size(px(theme.text_sm))
                .cursor_pointer()
                .child(label)
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.set_language(choice, cx);
                })),
        );
    }
    row
}

fn autosave_buttons(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut row = div().flex().gap(theme.space(1.0));
    for seconds in [0, 60, 180, 300] {
        let selected = app.autosave_interval_secs == seconds;
        row = row.child(
            div()
                .id(("autosave-interval", seconds as usize))
                .px(theme.space(1.5))
                .h(px(30.0))
                .flex()
                .items_center()
                .rounded(px(theme.radius_control))
                .bg(theme.well)
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .text_color(if selected { theme.accent } else { theme.text })
                .text_size(px(theme.text_sm))
                .cursor_pointer()
                .child(seconds.to_string())
                .on_click(cx.listener(move |this, _, _window, cx| {
                    this.set_autosave_interval(seconds, cx);
                })),
        );
    }
    row
}

fn unit_buttons(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let mut row = div().flex().flex_wrap().gap(theme.space(1.0));
    for units in Units::ALL {
        let selected = app.units == units;
        row = row.child(
            div()
                .id(("units", units as usize))
                .px(theme.space(1.5))
                .h(px(30.0))
                .flex()
                .items_center()
                .rounded(px(theme.radius_control))
                .bg(theme.well)
                .border_1()
                .border_color(if selected { theme.accent } else { theme.border })
                .text_color(if selected { theme.accent } else { theme.text })
                .text_size(px(theme.text_sm))
                .cursor_pointer()
                .child(units.symbol())
                .on_click(cx.listener(move |this, _, _window, cx| this.set_units(units, cx))),
        );
    }
    row
}

fn settings_label(theme: &crate::theme::Theme, label: &'static str) -> impl IntoElement {
    div()
        .text_size(px(theme.text_sm))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text_faint)
        .child(label)
}

fn theme_button(app: &Free3dApp, dark: bool, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let selected = app.theme.is_dark == dark;
    div()
        .id(if dark { "theme-dark" } else { "theme-light" })
        .flex_1()
        .h(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme.radius_control))
        .bg(theme.well)
        .border_1()
        .border_color(if selected { theme.accent } else { theme.border })
        .text_color(if selected { theme.accent } else { theme.text })
        .text_size(px(theme.text_md))
        .hover(|button| button.bg(theme.hover))
        .cursor_pointer()
        .child(if dark {
            crate::i18n::t("Dark")
        } else {
            crate::i18n::t("Light")
        })
        .on_click(cx.listener(move |this, _, _window, cx| {
            this.set_theme_variant(dark, cx);
        }))
}

fn file_button(
    app: &Free3dApp,
    label: &'static str,
    icon: &'static str,
    command: AppCommand,
    cx: &mut Context<Free3dApp>,
) -> impl IntoElement {
    let theme = &app.theme;
    div()
        .id(icon)
        .flex_1()
        .h(px(32.0))
        .flex()
        .items_center()
        .justify_center()
        .gap(theme.space(1.0))
        .rounded(px(theme.radius_control))
        .bg(theme.well)
        .border_1()
        .border_color(theme.border)
        .text_color(theme.text)
        .text_size(px(theme.text_md))
        .hover(|button| button.bg(theme.hover).border_color(theme.border_strong))
        .cursor_pointer()
        .child(glyph(theme, icon).size(px(theme.icon - 3.0)))
        .child(label)
        .on_click(cx.listener(move |this, _, window, cx| {
            this.dispatch(command, window, cx);
        }))
}
