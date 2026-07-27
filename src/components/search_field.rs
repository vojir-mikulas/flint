// SPDX-License-Identifier: GPL-3.0-or-later

//! `SearchField` — the combined search/filter control: several controls that
//! *read as one bordered field* rather than as a row of unrelated widgets.
//!
//! ```text
//! [ Contains ▾ │ acme corp……………………… 🕘 ]
//! [ amount ▾ │ >= ▾ │ 100……………………… + ]
//! ```
//!
//! The pattern kept getting rebuilt by hand — border, radius, height, the 1px
//! dividers, the focus/open border swap — once per shell, drifting a little each
//! time. This owns exactly that chrome and nothing else: every slot is an
//! arbitrary element the caller builds, so the component knows nothing about
//! modes, queries, or operators.
//!
//! Slots, in render order:
//! - [`slot`](SearchField::slot) — a leading control (typically a seamless
//!   [`Select`](crate::components::select::Select)). Each is followed by a
//!   divider, so several read as one segmented run.
//! - [`input`](SearchField::input) — the flexible middle that takes the leftover
//!   width. Padded by default; a surface that paints its own chrome (a one-line
//!   [`CodeEditor`](crate::components::code_editor::CodeEditor)) uses
//!   [`input_flush`](SearchField::input_flush).
//! - [`action`](SearchField::action) — a trailing affordance (a clear ✕, a
//!   history clock, a Run button). No divider: these read as icons *in* the
//!   field, not as another segment.
//! - [`overlay`](SearchField::overlay) — a dropdown anchored to the field. The
//!   field is `relative`, so a [`floating`](crate::components::floating::floating)
//!   child in relative mode flows from its top-left and escapes the clip.

use gpui::{div, prelude::*, AnyElement, App, Hsla, Pixels, SharedString, Window};

use crate::theme::ActiveTheme;

/// Default field height. Matches the browse/search toolbars this was extracted
/// from; a denser bar sets its own with [`SearchField::height`].
const DEFAULT_HEIGHT: Pixels = gpui::px(28.);
/// Default minimum width, so a field in a flexible row can't collapse to nothing.
const DEFAULT_MIN_WIDTH: Pixels = gpui::px(180.);
/// Default horizontal padding inside the input slot.
const DEFAULT_INPUT_PAD: Pixels = gpui::px(8.);
/// Smallest the input slot may shrink to before the row wraps or scrolls.
const INPUT_MIN_WIDTH: Pixels = gpui::px(60.);

#[derive(IntoElement)]
pub struct SearchField {
    id: SharedString,
    slots: Vec<AnyElement>,
    input: Option<AnyElement>,
    actions: Vec<AnyElement>,
    overlay: Option<AnyElement>,
    height: Pixels,
    min_width: Pixels,
    input_pad: Pixels,
    background: Option<Hsla>,
    active: bool,
}

impl SearchField {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            slots: Vec::new(),
            input: None,
            actions: Vec::new(),
            overlay: None,
            height: DEFAULT_HEIGHT,
            min_width: DEFAULT_MIN_WIDTH,
            input_pad: DEFAULT_INPUT_PAD,
            background: None,
            active: false,
        }
    }

    /// Add a leading control, followed by a divider. Call several times to build
    /// a segmented run (`column ▾ │ operator ▾ │ …`).
    pub fn slot(mut self, element: impl IntoElement) -> Self {
        self.slots.push(element.into_any_element());
        self
    }

    /// The flexible middle: the text box, taking the leftover width.
    pub fn input(mut self, element: impl IntoElement) -> Self {
        self.input = Some(element.into_any_element());
        self
    }

    /// Drop the input slot's padding, for a surface that brings its own (a
    /// one-line `CodeEditor`). Such a surface usually also paints its own
    /// background, so pair this with [`background`](Self::background).
    pub fn input_flush(mut self) -> Self {
        self.input_pad = gpui::px(0.);
        self
    }

    /// Add a trailing affordance inside the field. No divider is drawn.
    pub fn action(mut self, element: impl IntoElement) -> Self {
        self.actions.push(element.into_any_element());
        self
    }

    /// Hang a floating surface off the field (a dropdown list). Pass a
    /// [`floating`](crate::components::floating::floating) in relative mode; the
    /// field is the positioned parent it flows from.
    pub fn overlay(mut self, element: impl IntoElement) -> Self {
        self.overlay = Some(element.into_any_element());
        self
    }

    /// "Something is open or focused here": paints the strong border. The caller
    /// owns the notion (an open dropdown, a focused input), so this is a plain
    /// flag rather than the component tracking focus itself.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn height(mut self, height: Pixels) -> Self {
        self.height = height;
        self
    }

    pub fn min_width(mut self, width: Pixels) -> Self {
        self.min_width = width;
        self
    }

    /// Override the field's background. Defaults to `bg_input`; a slot that
    /// paints its own tone matches it here so the field isn't two-toned.
    pub fn background(mut self, background: Hsla) -> Self {
        self.background = Some(background);
        self
    }
}

impl RenderOnce for SearchField {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let border = theme.border;
        // The dividers are shorter than the field so they read as separators
        // inside one control rather than as the seams between three of them.
        let divider_h = self.height - gpui::px(12.);

        let mut field = div()
            .id(self.id)
            // The overlay is placed against this element's origin.
            .relative()
            .flex()
            .flex_1()
            .items_center()
            .min_w(self.min_width)
            .h(self.height)
            .rounded(theme.radius)
            .bg(self.background.unwrap_or(theme.bg_input))
            .border_1()
            .border_color(if self.active {
                theme.border_strong
            } else {
                border
            })
            // Keeps a long value from painting past the rounded corners. The
            // overlay is `deferred`, so it still escapes this clip.
            .overflow_hidden();

        for slot in self.slots {
            field = field.child(slot).child(
                div()
                    .flex_shrink_0()
                    .w(gpui::px(1.))
                    .h(divider_h)
                    .bg(border),
            );
        }
        field = match self.input {
            Some(input) => field.child(
                div()
                    .flex_1()
                    .min_w(INPUT_MIN_WIDTH)
                    .h_full()
                    .flex()
                    .items_center()
                    .px(self.input_pad)
                    .child(input),
            ),
            // No input (a field that is only pickers): the actions still sit at
            // the trailing edge rather than bunching against the last slot.
            None => field.child(div().flex_1()),
        };
        field.children(self.actions).children(self.overlay)
    }
}
