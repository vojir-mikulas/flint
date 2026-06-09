// SPDX-License-Identifier: GPL-3.0-or-later

//! `Toast` - a single notification pill. Stacking, positioning and auto-dismiss
//! are the caller's concern; this is just the visual.

use gpui::{div, prelude::*, App, Hsla, SharedString, Window};

use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToastVariant {
    #[default]
    Info,
    Success,
    Error,
}

#[derive(IntoElement)]
pub struct Toast {
    message: SharedString,
    variant: ToastVariant,
}

impl Toast {
    pub fn new(message: impl Into<SharedString>) -> Self {
        Self {
            message: message.into(),
            variant: ToastVariant::default(),
        }
    }

    pub fn variant(mut self, variant: ToastVariant) -> Self {
        self.variant = variant;
        self
    }
}

fn dot_color(variant: ToastVariant, theme: &crate::Theme) -> Hsla {
    match variant {
        ToastVariant::Info => theme.accent,
        ToastVariant::Success => theme.green,
        ToastVariant::Error => theme.red,
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let dot = dot_color(self.variant, theme);

        div()
            .flex()
            .items_center()
            .gap_2p5()
            .min_w(gpui::px(220.))
            .px_3()
            .py_2p5()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(gpui::px(7.))
            .shadow_lg()
            .text_sm()
            .text_color(theme.text)
            .child(
                div()
                    .size(gpui::px(8.))
                    .rounded_full()
                    .bg(dot)
                    .flex_shrink_0(),
            )
            .child(self.message)
    }
}
