//! Sketch-only floating constraint actions on the right edge.

use gpui::{Context, div, prelude::*, px, rgba};

use crate::{
    app::DuctileApp,
    commands::SketchConstraintKind,
    ui::{self, tip},
};

/// Builds the right-side constraint strip while a sketch is active.
pub fn render(app: &DuctileApp, cx: &mut Context<DuctileApp>) -> Option<impl IntoElement> {
    app.document.read(cx).active_sketch?;
    let theme = &app.theme;
    Some(
        ui::surface(theme)
            .absolute()
            .right(theme.space(3.0))
            .bottom(theme.space(3.0))
            .flex()
            .flex_col()
            .p(theme.space(1.0))
            .gap(px(2.0))
            .children(
                SketchConstraintKind::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, kind)| {
                        let enabled = app.constraint_enabled(kind, cx);
                        let conflict = app.last_constraint_conflict == Some(kind);
                        let foreground = if conflict {
                            rgba(0xff5c5cff).into()
                        } else if enabled {
                            theme.text
                        } else {
                            theme.text_muted
                        };
                        div()
                            .id(("sketch-constraint", index))
                            .size(px(theme.control))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(theme.radius_control))
                            .text_size(px(theme.text_md + 1.0))
                            .text_color(foreground)
                            .when(enabled, |button| {
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.bg(theme.hover))
                                    .active(|style| style.bg(theme.active))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.apply_sketch_constraint(kind, cx)
                                    }))
                            })
                            .when(!enabled, |button| button.opacity(0.38))
                            .tooltip(tip(theme, kind.tooltip(), None))
                            .child(kind.mark())
                    }),
            ),
    )
}
