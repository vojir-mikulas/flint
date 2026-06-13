// SPDX-License-Identifier: GPL-3.0-or-later

//! `NumberInput` - a compact numeric stepper: a `−` button, an editable value
//! field, and a `+` button, all inside one bordered box (the Zed style).
//! Stateful: a [`Render`] view held in an `Entity`, wrapping a bare
//! [`TextInput`] for the typeable middle. Emits [`NumberInputEvent::Change`]
//! when the value is committed — a stepper press, Enter, or the field losing
//! focus — so the owner never parses strings itself. Typing is *not* committed
//! live: clamping each keystroke would fight a half-typed number (you could
//! never reach `14` past a minimum of `8`). Clamps to `[min, max]` and snaps to
//! the displayed precision.

use gpui::{
    div, prelude::*, px, App, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    Subscription, TextAlign, Window,
};

use crate::components::text_input::{TextInput, TextInputEvent};
use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

/// Emitted when the numeric value changes, so the owner can persist it.
#[derive(Clone, Copy, Debug)]
pub enum NumberInputEvent {
    /// The committed (clamped) value. Fires on a stepper press, on Enter, and
    /// when the field loses focus — never mid-typing.
    Change(f64),
}

pub struct NumberInput {
    id: ElementId,
    input: Entity<TextInput>,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    decimals: usize,
    _subscription: Subscription,
    /// Installed on the first render (when a `Window` exists): commits the typed
    /// value when the field loses focus.
    blur_subscription: Option<Subscription>,
}

impl NumberInput {
    pub fn new(id: impl Into<ElementId>, cx: &mut Context<Self>) -> Self {
        // The middle is a bare field: no chrome of its own, no tab stop (the box
        // owns the focus ring; Tab should skip straight past the stepper), and the
        // value sits centered between the steppers.
        // A tab stop so Tab reaches the typeable field (the steppers are mouse-only);
        // landing here focuses the number for direct entry, Enter/blur commits.
        let input = cx.new(|cx| {
            TextInput::new(cx)
                .bare()
                .tab_stop(true)
                .align(TextAlign::Center)
        });

        // Commit is deferred, not live: clamping every keystroke would fight the
        // user mid-number (typing "14" snaps "1" up to the minimum). Instead we
        // only translate the text into a clamped value on Enter — and on blur,
        // wired up in `render` once a `Window` is available. Esc discards the edit.
        let subscription =
            cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| match event {
                TextInputEvent::Submit => this.commit(cx),
                TextInputEvent::Cancel => {
                    let value = this.value;
                    this.set_value(value, cx);
                }
                TextInputEvent::Change => {}
            });

        Self {
            id: id.into(),
            input,
            value: 0.0,
            min: f64::NEG_INFINITY,
            max: f64::INFINITY,
            step: 1.0,
            decimals: 0,
            _subscription: subscription,
            blur_subscription: None,
        }
    }

    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        self.step = step;
        self
    }

    /// Fractional digits shown in the field (e.g. `2` → `15.00`).
    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }

    pub fn value(&self) -> f64 {
        self.value
    }

    /// Set the value programmatically and reformat the field. Does **not** emit
    /// [`NumberInputEvent::Change`] — the owner already knows.
    pub fn set_value(&mut self, value: f64, cx: &mut Context<Self>) {
        self.value = self.normalize(value);
        let text = self.format();
        self.input
            .update(cx, |input, cx| input.set_content(text, cx));
        cx.notify();
    }

    /// Clamp into range and snap to the displayed precision, so the stored value
    /// never disagrees with what the field shows.
    fn normalize(&self, value: f64) -> f64 {
        let clamped = value.clamp(self.min, self.max);
        let factor = 10f64.powi(self.decimals as i32);
        (clamped * factor).round() / factor
    }

    fn format(&self) -> String {
        format!("{:.*}", self.decimals, self.value)
    }

    /// Parse the field's current text, clamp it, rewrite the field to the
    /// canonical form, and broadcast the value. Called on Enter and on blur.
    /// Junk or out-of-range text ("", "-", "1.") snaps back to the last good
    /// value. Only emits when the value actually changed, so a blur with no edit
    /// (or a re-commit of the same number) stays quiet.
    fn commit(&mut self, cx: &mut Context<Self>) {
        let raw = self.input.read(cx).content();
        let parsed = raw.trim().parse::<f64>().ok();
        let next = parsed.map(|v| self.normalize(v)).unwrap_or(self.value);
        let changed = next != self.value;
        self.value = next;
        // Always reformat: even when the value is unchanged, the field may hold
        // raw text ("08", "14.") that should settle to canonical form.
        let text = self.format();
        self.input
            .update(cx, |input, cx| input.set_content(text, cx));
        if changed {
            cx.emit(NumberInputEvent::Change(next));
        }
        cx.notify();
    }

    fn nudge(&mut self, delta: f64, cx: &mut Context<Self>) {
        let next = self.normalize(self.value + delta);
        if next == self.value {
            return;
        }
        self.value = next;
        let text = self.format();
        self.input
            .update(cx, |input, cx| input.set_content(text, cx));
        cx.emit(NumberInputEvent::Change(next));
        cx.notify();
    }
}

impl Focusable for NumberInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<NumberInputEvent> for NumberInput {}

impl Render for NumberInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus_handle(cx).is_focused(window);
        let step = self.step;

        // Commit-on-blur needs a Window, so it can't be set up in `new`; install it
        // the first time we render (the handle exists by then). Held in `self`, so
        // it lives as long as the component.
        if self.blur_subscription.is_none() {
            let handle = self.focus_handle(cx);
            self.blur_subscription =
                Some(cx.on_blur(&handle, window, |this, _, cx| this.commit(cx)));
        }

        // One stepper button: a square, full-height tap target that brightens on
        // hover. `glyph` is "−" / "+"; `delta` its signed step.
        let button = |key: &'static str, glyph: &'static str, delta: f64| {
            div()
                .id(key)
                .flex()
                .items_center()
                .justify_center()
                .w(px(26.))
                .h_full()
                .text_color(theme.text_muted)
                .cursor_pointer()
                .hover(|s| s.bg(theme.bg_hover).text_color(theme.text))
                .on_click(cx.listener(move |this, _, _, cx| this.nudge(delta, cx)))
                .child(glyph)
        };

        let mut row = div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .h(px(28.))
            .rounded(theme.radius)
            .bg(theme.bg_input)
            .border_1()
            .border_color(if focused {
                theme.border_strong
            } else {
                theme.border
            })
            .overflow_hidden()
            .text_size(theme.font_size)
            .child(button("num-dec", "−", -step))
            .child(div().w(px(1.)).h_full().bg(theme.border))
            .child(
                // The typeable middle. Fixed width keeps the box from jumping as
                // the digit count changes; the field centers its own text.
                div()
                    .w(px(56.))
                    .px_1()
                    .text_color(theme.text)
                    .child(self.input.clone()),
            )
            .child(div().w(px(1.)).h_full().bg(theme.border))
            .child(button("num-inc", "+", step));

        if focused {
            row = row.focus_ring(cx);
        }
        row
    }
}
