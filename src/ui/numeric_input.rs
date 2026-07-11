//! Compact, caret-at-end numeric expression input for inline dimensions.

use std::collections::HashMap;

use gpui::{
    Context, EventEmitter, FocusHandle, KeyDownEvent, Render, SharedString, Window, div,
    prelude::*, px,
};

use crate::{theme::Theme, units::Units};

use super::expr;

/// Result emitted by a [`NumericInput`].
#[derive(Clone, Debug, PartialEq)]
pub enum NumericInputEvent {
    /// Enter was pressed with a valid expression result.
    Commit {
        /// Evaluated finite number.
        value: f64,
        /// Trimmed source text entered by the user.
        expression: String,
    },
    /// Escape was pressed.
    Cancel,
}

/// Minimal single-line expression editor with its caret fixed at the end.
pub struct NumericInput {
    focus_handle: FocusHandle,
    text: String,
    suffix: SharedString,
    theme: Theme,
    variables: HashMap<String, f64>,
    error: Option<String>,
    units: Option<Units>,
}

impl NumericInput {
    /// Creates an editor seeded with `text` and an optional right-side suffix.
    pub fn new(
        text: impl Into<String>,
        suffix: impl Into<SharedString>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: text.into(),
            suffix: suffix.into(),
            theme,
            variables: HashMap::new(),
            error: None,
            units: None,
        }
    }

    /// Treats the evaluated value as a length in `units` and emits millimetres.
    pub fn with_units(mut self, units: Units) -> Self {
        self.suffix = units.symbol().into();
        self.units = Some(units);
        self
    }

    /// Creates an editor whose expressions can resolve the supplied variables.
    pub fn new_with_variables(
        text: impl Into<String>,
        suffix: impl Into<SharedString>,
        theme: Theme,
        variables: HashMap<String, f64>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut input = Self::new(text, suffix, theme, cx);
        input.variables = variables;
        input
    }

    /// Focuses the editor so its first rendered frame accepts key input.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        let resolver = |name: &str| self.variables.get(name).copied();
        let value = if self.variables.is_empty() {
            expr::eval(&self.text)
        } else {
            expr::eval_with(&self.text, &resolver)
        };
        if let Some(value) = value {
            let value = if expr::contains_identifier(&self.text) {
                value
            } else {
                self.units.map_or(value, |units| units.parse_value(value))
            };
            self.error = None;
            cx.emit(NumericInputEvent::Commit {
                value,
                expression: self.text.trim().to_owned(),
            });
        } else {
            self.error = Some(
                expr::first_unknown_identifier(&self.text, &resolver)
                    .map(|name| format!("未定义变量 {name}"))
                    .unwrap_or_else(|| "表达式无效".to_owned()),
            );
            cx.notify();
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        if key.eq_ignore_ascii_case("enter") {
            self.commit(cx);
        } else if key.eq_ignore_ascii_case("escape") {
            cx.emit(NumericInputEvent::Cancel);
        } else if key.eq_ignore_ascii_case("backspace") {
            self.text.pop();
            self.error = None;
            cx.notify();
        } else if !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.control
            && let Some(text) = &event.keystroke.key_char
        {
            self.text.extend(text.chars().filter(|character| {
                character.is_ascii_digit()
                    || character.is_ascii_alphabetic()
                    || *character == '_'
                    || matches!(character, '.' | '-' | '+' | '*' | '/' | '(' | ')')
            }));
            self.error = None;
            cx.notify();
        }
        cx.stop_propagation();
    }
}

impl EventEmitter<NumericInputEvent> for NumericInput {}

impl Render for NumericInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        let error = self.error.clone();
        div()
            .id("numeric-input")
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(Self::key_down))
            .min_w(px(76.0))
            .min_h(px(28.0))
            .px(theme.space(2.0))
            .flex()
            .flex_wrap()
            .items_center()
            .gap(theme.space(1.0))
            .rounded(px(theme.radius_control))
            .bg(theme.elevated)
            .border_1()
            .border_color(theme.accent)
            .shadow(theme.shadow.clone())
            .text_color(theme.text)
            .text_size(px(theme.text_md))
            .child(self.text.clone())
            .child(div().w(px(1.5)).h(px(14.0)).bg(theme.accent))
            .child(
                div()
                    .ml_auto()
                    .min_w(theme.space(1.0))
                    .text_color(theme.text_muted)
                    .text_size(px(theme.text_sm))
                    .child(self.suffix.clone()),
            )
            .child(
                div()
                    .id("numeric-apply")
                    .ml(theme.space(1.0))
                    .px(theme.space(1.5))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .bg(theme.accent)
                    .text_color(theme.on_accent)
                    .cursor_pointer()
                    .child("Apply")
                    .on_click(cx.listener(|input, _, _window, cx| input.commit(cx))),
            )
            .when_some(error, |input, error| {
                input.child(
                    div()
                        .w_full()
                        .text_size(px(theme.text_sm))
                        .text_color(theme.axis_x)
                        .child(error),
                )
            })
    }
}
