// SPDX-License-Identifier: GPL-3.0-or-later

//! `Tooltip` - a small elevated label shown on hover. GPUI's `.tooltip(..)`
//! wants a closure returning an `AnyView`, so a tooltip must be a view;
//! [`Tooltip::text`] returns a ready-made builder closure.

use gpui::{div, prelude::*, AnyView, App, SharedString, Window};

use crate::theme::ActiveTheme;

pub struct Tooltip {
    text: SharedString,
    /// When set, render in the theme's danger red - border and text - to flag a
    /// risky/"at your own risk" affordance rather than a neutral hint.
    danger: bool,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            danger: false,
        }
    }

    /// A ready-made builder closure for [`gpui::InteractiveElement::tooltip`].
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_window, cx| cx.new(|_| Tooltip::new(text.clone())).into()
    }

    /// Like [`Tooltip::text`], but red-styled to warn about a risky choice.
    pub fn danger(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_window, cx| {
            cx.new(|_| Tooltip {
                text: text.clone(),
                danger: true,
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (border, text_color) = if self.danger {
            (theme.red, theme.red)
        } else {
            (theme.border_strong, theme.text)
        };
        div()
            .px_2()
            .py_1()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(border)
            .rounded(theme.radius_sm)
            .shadow_lg()
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size_xs())
            .text_color(text_color)
            .child(self.text.clone())
    }
}
