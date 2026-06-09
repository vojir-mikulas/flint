// SPDX-License-Identifier: GPL-3.0-or-later

//! `Tooltip` - a small elevated label shown on hover. GPUI's `.tooltip(..)`
//! wants a closure returning an `AnyView`, so a tooltip must be a view;
//! [`Tooltip::text`] returns a ready-made builder closure.

use gpui::{div, prelude::*, AnyView, App, SharedString, Window};

use crate::theme::ActiveTheme;

pub struct Tooltip {
    text: SharedString,
}

impl Tooltip {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }

    /// A ready-made builder closure for [`gpui::InteractiveElement::tooltip`].
    pub fn text(
        text: impl Into<SharedString>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let text = text.into();
        move |_window, cx| cx.new(|_| Tooltip::new(text.clone())).into()
    }
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .px_2()
            .py_1()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(theme.radius_sm)
            .shadow_lg()
            .text_xs()
            .text_color(theme.text)
            .child(self.text.clone())
    }
}
