//! Floating chrome layered over the 3D viewport.
//!
//! Every panel in this module positions itself absolutely so the viewport
//! stays full-window underneath and keeps receiving pointer, scroll and pinch
//! events everywhere the chrome does not paint. Submodules render into
//! [`crate::app::Free3dApp`]'s context and emit [`crate::commands::AppCommand`]s
//! rather than mutating state directly.

pub mod adaptive_menu;
pub mod command_search;
pub mod constraints_panel;
pub mod drawing_canvas;
pub mod expr;
pub mod icons;
pub mod inspection_card;
pub mod modes;
pub mod numeric_input;
pub mod panels;
pub mod tool_strip;
pub mod top_bar;
pub mod view_cluster;

use gpui::{
    AnyView, App, FontWeight, InteractiveElement, SharedString, Stateful,
    StatefulInteractiveElement, Svg, Window, div, prelude::*, px, svg,
};

use crate::theme::Theme;

/// Builds a monochrome icon element.
///
/// gpui's `svg` element only paints when its OWN style carries a text colour
/// (parent inheritance does not apply), so a sensible default is set here;
/// chain `.text_color(..)` to override.
pub fn glyph(theme: &Theme, name: &str) -> Svg {
    svg()
        .path(icons::path(name))
        .size(px(theme.icon))
        .flex_shrink_0()
        .text_color(theme.text_muted)
}

/// A floating surface: translucent fill, hairline border, rounded, soft shadow.
pub fn surface(theme: &Theme) -> gpui::Div {
    surface_with(theme, theme.panel)
}

/// A brighter floating surface for popovers and menus that sit above panels.
pub fn surface_elevated(theme: &Theme) -> gpui::Div {
    surface_with(theme, theme.elevated)
}

fn surface_with(theme: &Theme, bg: gpui::Hsla) -> gpui::Div {
    div()
        .bg(bg)
        .border_1()
        .border_color(theme.border)
        .rounded(px(theme.radius_panel))
        .shadow(theme.shadow.clone())
        .text_color(theme.text)
}

/// A square ghost icon button with hover / active / selected states.
///
/// The caller attaches `.on_click(..)`, a tooltip and any flyout child.
pub fn icon_button(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    name: &str,
    active: bool,
) -> Stateful<gpui::Div> {
    let fg = if active {
        theme.accent
    } else {
        theme.text_muted
    };
    let hover_fg = if active {
        theme.accent_hover
    } else {
        theme.text
    };
    div()
        .id(id)
        .group("icon-button")
        .size(px(theme.control))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(theme.radius_control))
        .text_color(fg)
        .when(active, |d| d.bg(theme.accent_wash))
        .hover(move |s| s.bg(theme.hover).text_color(hover_fg))
        .active(|s| s.bg(theme.active))
        .cursor_pointer()
        .child(
            glyph(theme, name)
                .text_color(fg)
                .group_hover("icon-button", move |s| s.text_color(hover_fg)),
        )
}

/// A one-pixel divider used to separate control groups.
pub fn divider(theme: &Theme, vertical: bool) -> gpui::Div {
    let mut d = div().bg(theme.border);
    if vertical {
        d = d.w(px(1.0)).h(px(18.0));
    } else {
        d = d.h(px(1.0)).w_full();
    }
    d
}

/// A hover tooltip showing a tool name and optional shortcut chip.
pub struct Tooltip {
    theme: Theme,
    title: SharedString,
    shortcut: Option<SharedString>,
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        surface_elevated(theme)
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.space(2.0))
            .py(theme.space(1.5))
            .px(theme.space(2.5))
            .text_size(px(theme.text_sm + 1.0))
            .child(self.title.clone())
            .when_some(self.shortcut.clone(), |el, shortcut| {
                el.child(
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
    }
}

/// Returns a tooltip builder closure for [`InteractiveElement::tooltip`].
pub fn tip(
    theme: &Theme,
    title: impl Into<SharedString>,
    shortcut: Option<&str>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let theme = theme.clone();
    let title = title.into();
    let shortcut = shortcut.map(SharedString::from);
    move |_window, cx| {
        let theme = theme.clone();
        let title = title.clone();
        let shortcut = shortcut.clone();
        cx.new(|_| Tooltip {
            theme,
            title,
            shortcut,
        })
        .into()
    }
}
