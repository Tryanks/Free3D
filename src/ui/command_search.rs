//! Top-center command-search palette.

use gpui::{Context, FontWeight, div, prelude::*, px};

use crate::{
    app::DuctileApp,
    commands::SearchCommand,
    theme::Theme,
    ui::{self, glyph},
};

/// Case-insensitive subsequence matching used by command search.
pub fn fuzzy_subsequence(query: &str, candidate: &str) -> bool {
    let mut candidate = candidate.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|needle| candidate.by_ref().any(|character| character == needle))
}

/// Returns registry commands matching `query` in display order.
pub fn filtered_commands(query: &str) -> Vec<SearchCommand> {
    SearchCommand::all()
        .into_iter()
        .filter(|command| {
            let label = command.label();
            fuzzy_subsequence(query.trim(), label)
                || fuzzy_subsequence(query.trim(), crate::i18n::english_key(label))
        })
        .collect()
}

/// Renders the palette when it is open.
pub fn render(app: &DuctileApp, cx: &mut Context<DuctileApp>) -> Option<impl IntoElement> {
    app.show_command_search.then(|| {
        let theme = &app.theme;
        let commands = filtered_commands(&app.command_query);
        let mut list = div()
            .id("command-search-results")
            .max_h(px(420.0))
            .overflow_scroll()
            .flex()
            .flex_col()
            .gap(px(1.0));
        for (index, command) in commands.into_iter().enumerate() {
            list = list.child(command_row(
                theme,
                command,
                index,
                app.command_highlight,
                cx,
            ));
        }

        div()
            .absolute()
            .top(px(66.0))
            .left(px(0.0))
            .right(px(0.0))
            .flex()
            .justify_center()
            .child(
                ui::surface_elevated(theme)
                    .id("command-search")
                    .track_focus(&app.command_focus)
                    .on_key_down(cx.listener(DuctileApp::command_search_key_down))
                    .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                        this.close_command_search(window, cx);
                    }))
                    .w(px(480.0))
                    .p(theme.space(2.0))
                    .flex()
                    .flex_col()
                    .gap(theme.space(1.5))
                    .child(
                        div()
                            .h(px(38.0))
                            .px(theme.space(2.0))
                            .flex()
                            .items_center()
                            .gap(theme.space(2.0))
                            .rounded(px(theme.radius_control))
                            .bg(theme.well)
                            .border_1()
                            .border_color(theme.accent)
                            .text_size(px(theme.text_lg))
                            .child(
                                div()
                                    .text_color(theme.text_muted)
                                    .child(glyph(theme, "search")),
                            )
                            .child(if app.command_query.is_empty() {
                                div()
                                    .text_color(theme.text_faint)
                                    .child(crate::i18n::t("Search commands…"))
                            } else {
                                div()
                                    .text_color(theme.text)
                                    .child(app.command_query.clone())
                            })
                            .child(div().w(px(1.5)).h(px(16.0)).bg(theme.accent)),
                    )
                    .child(list),
            )
    })
}

fn command_row(
    theme: &Theme,
    command: SearchCommand,
    index: usize,
    highlight: usize,
    cx: &mut Context<DuctileApp>,
) -> impl IntoElement {
    div()
        .id(("search-command", index))
        .h(px(36.0))
        .px(theme.space(2.0))
        .flex()
        .items_center()
        .gap(theme.space(2.0))
        .rounded(px(theme.radius_control))
        .when(index == highlight, |row| row.bg(theme.accent_wash))
        .hover(|row| row.bg(theme.hover))
        .cursor_pointer()
        .child(
            div()
                .text_color(theme.text_muted)
                .child(glyph(theme, command.icon())),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(theme.text_md))
                .child(command.label()),
        )
        .when_some(command.shortcut(), |row, shortcut| {
            row.child(
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
                    .child(shortcut),
            )
        })
        .on_click(cx.listener(move |this, _, window, cx| {
            this.execute_search_command(command, window, cx);
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_subsequence_is_case_insensitive_and_ordered() {
        assert!(fuzzy_subsequence("exd", "Extrude"));
        assert!(fuzzy_subsequence("MVRT", "Move / Rotate"));
        assert!(fuzzy_subsequence("", "Anything"));
        assert!(!fuzzy_subsequence("etx", "Extrude"));
        assert!(!fuzzy_subsequence("zoom", "Isometric view"));
    }
}
