// SPDX-License-Identifier: GPL-3.0-or-later

//! `Modal` - a centered dialog over a dimming scrim. Body and footer are
//! arbitrary `impl IntoElement`. Place it last in a positioned container (or
//! behind a [`gpui::deferred`] layer) so it paints above the rest of the UI.

use gpui::{div, prelude::*, AnyElement, App, ElementId, MouseButton, SharedString, Window};

use crate::theme::ActiveTheme;

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    title: Option<SharedString>,
    width: gpui::Pixels,
    body: Vec<AnyElement>,
    footer: Option<AnyElement>,
    on_close: Option<CloseHandler>,
}

impl Modal {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            title: None,
            width: gpui::px(540.),
            body: Vec::new(),
            footer: None,
            on_close: None,
        }
    }

    /// Without a title the header is omitted.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn width(mut self, width: gpui::Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// Invoked by the × button and a scrim click.
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl RenderOnce for Modal {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let on_close = self.on_close.map(std::rc::Rc::new);

        let header = self.title.map(|title| {
            let close = on_close.clone();
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .px_4()
                .py_3p5()
                .border_b_1()
                .border_color(theme.border_soft)
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .id("modal-close")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(gpui::px(24.))
                        .rounded(theme.radius_sm)
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover).text_color(theme.text))
                        .child("✕")
                        .when_some(close, |this, close| {
                            // Focusable so Tab reaches it and Enter/Space close.
                            this.tab_index(0)
                                .focus(|s| s.bg(theme.bg_hover).text_color(theme.text))
                                .on_click(move |_, window, cx| close(window, cx))
                        }),
                )
        });

        let footer = self.footer.map(|footer| {
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .px_4()
                .py_3()
                .border_t_1()
                .border_color(theme.border_soft)
                .bg(theme.bg_bar)
                // Match the panel's rounding so the bar's own fill doesn't paint
                // sharp corners into the rounded bottom (overflow_hidden clips
                // content, not a child's background corners).
                .rounded_b(gpui::px(9.))
                .child(footer)
        });

        let panel = div()
            .occlude()
            .flex()
            .flex_col()
            .w(self.width)
            .max_h(gpui::relative(0.88))
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(gpui::px(9.))
            .shadow_lg()
            .overflow_hidden()
            .when_some(header, |this, header| this.child(header))
            .child(
                div()
                    .id("modal-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .children(self.body),
            )
            .when_some(footer, |this, footer| this.child(footer));

        div()
            .id(self.id)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            // Block all mouse interaction with the UI behind the scrim, so a
            // click that dismisses the modal can't also fire an element under it.
            .occlude()
            .when_some(on_close, |this, close| {
                this.on_mouse_down(MouseButton::Left, move |_, window, cx| close(window, cx))
            })
            .child(panel)
    }
}
