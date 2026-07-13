//! Floating adaptive menu: a ranked, selection-driven tool list beside the
//! left tool strip.
//!
//! When the selection is non-empty and any tool applies to it, a compact card
//! appears just right of the strip listing the most relevant tools. Clicking an
//! entry fires the same [`AppCommand::ActivateTool`] the strip would. The card
//! is content-sized and absolutely positioned, so it never intercepts viewport
//! events outside its own bounds; it vanishes when the selection clears or a
//! modeling pointer-drag begins.

use gpui::{Context, div, prelude::*, px};

use crate::{
    adaptive::adaptive_tools,
    app::Free3dApp,
    commands::{AppCommand, ToolId},
    ui::{self, glyph, tool_strip},
};

/// Builds the adaptive menu, or nothing when no tool applies to the selection.
pub fn render(app: &Free3dApp, cx: &mut Context<Free3dApp>) -> Option<impl IntoElement> {
    if app.viewport.read(cx).interaction_in_progress() {
        return None;
    }

    let theme = &app.theme;
    let tools = adaptive_tools(&app.document.read(cx).selection, app.document.read(cx));
    if tools.is_empty() {
        return None;
    }

    // Anchor the card to the right of the left rail (and any open Items /
    // History panels, which share the rail's width) so the two never overlap.
    let rail_top = tool_strip::rail_top(theme);
    let left = tool_strip::LEFT_INSET + tool_strip::LEFT_WIDTH + f32::from(theme.space(2.0));

    let mut card = ui::surface_elevated(theme)
        .absolute()
        .left(px(left))
        .top(px(rail_top))
        .w(px(196.0))
        .flex()
        .flex_col()
        .p(theme.space(1.0))
        .gap(px(1.0));

    for tool in tools {
        card = card.child(entry(app, tool, cx));
    }

    Some(card)
}

/// One ranked tool row inside the adaptive card.
fn entry(app: &Free3dApp, tool: ToolId, cx: &mut Context<Free3dApp>) -> impl IntoElement {
    let theme = &app.theme;
    let active = app.active_tool == Some(tool);
    let fg = if active {
        theme.accent
    } else {
        theme.text_muted
    };

    div()
        .id(("adaptive", tool as usize))
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
        .on_click(cx.listener(move |this, _, window, cx| {
            this.dispatch(AppCommand::ActivateTool(tool), window, cx)
        }))
}
