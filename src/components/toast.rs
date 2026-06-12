// SPDX-License-Identifier: GPL-3.0-or-later

//! `Toast` - a single notification pill. Stacking, positioning and auto-dismiss
//! are the caller's concern; this is just the visual. Optional affordances: a
//! trailing close (`✕`) button and a thin progress bar under the message (used
//! for long-running operations like an export). Both stay domain-free — the
//! caller owns the lifecycle, IDs, and timers.

use gpui::{div, prelude::*, relative, App, Hsla, SharedString, Window};

use crate::theme::ActiveTheme;

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToastVariant {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

#[derive(IntoElement)]
pub struct Toast {
    message: SharedString,
    variant: ToastVariant,
    on_close: Option<CloseHandler>,
    progress: Option<f32>,
}

impl Toast {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            variant: ToastVariant::default(),
            on_close: None,
            progress: None,
        }
    }

    pub fn variant(mut self, variant: ToastVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Render a trailing `✕` button wired to `handler` (dismiss / cancel).
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }

    /// Render a thin progress bar under the message. `fraction` is clamped to
    /// `0.0..=1.0`.
    pub fn progress(mut self, fraction: f32) -> Self {
        self.progress = Some(fraction.clamp(0.0, 1.0));
        self
    }
}

fn dot_color(variant: ToastVariant, theme: &crate::Theme) -> Hsla {
    match variant {
        ToastVariant::Info => theme.accent,
        ToastVariant::Success => theme.green,
        ToastVariant::Warning => theme.yellow,
        ToastVariant::Error => theme.red,
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let dot = dot_color(self.variant, theme);

        let row = div()
            .flex()
            .items_center()
            .gap_2p5()
            .child(
                div()
                    .size(gpui::px(8.))
                    .rounded_full()
                    .bg(dot)
                    .flex_shrink_0(),
            )
            .child(div().flex_1().child(self.message))
            .when_some(self.on_close, |row, handler| {
                row.child(
                    div()
                        .id("toast-close")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(gpui::px(20.))
                        .flex_shrink_0()
                        .rounded(theme.radius_sm)
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover).text_color(theme.text))
                        .tab_index(0)
                        .focus(|s| s.bg(theme.bg_hover).text_color(theme.text))
                        .child("✕")
                        .on_click(move |_, window, cx| handler(window, cx)),
                )
            });

        div()
            .flex()
            .flex_col()
            .gap_2()
            .min_w(gpui::px(220.))
            .px_3()
            .py_2p5()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(gpui::px(7.))
            .shadow_lg()
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size)
            .text_color(theme.text)
            .child(row)
            .when_some(self.progress, |card, fraction| {
                card.child(
                    div()
                        .h(gpui::px(3.))
                        .w_full()
                        .rounded(gpui::px(3.))
                        .bg(theme.bg_input)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded(gpui::px(3.))
                                .bg(dot),
                        ),
                )
            })
    }
}
