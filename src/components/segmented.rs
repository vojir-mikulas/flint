// SPDX-License-Identifier: GPL-3.0-or-later

//! A single-select segmented control. Stateless: the caller owns the selected
//! index and is notified of clicks via [`on_select`](Segmented::on_select).

use gpui::{div, prelude::*, App, SharedString, Window};

use crate::theme::ActiveTheme;

type SelectHandler = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Segmented {
    id: SharedString,
    segments: Vec<SharedString>,
    selected: usize,
    on_select: Option<SelectHandler>,
}

impl Segmented {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            segments: Vec::new(),
            selected: 0,
            on_select: None,
        }
    }

    pub fn segment(mut self, label: impl Into<SharedString>) -> Self {
        self.segments.push(label.into());
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Segmented {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected;
        let focus_bg = theme.bg_hover;
        let focus_fg = theme.text;
        let on_select = self.on_select.map(std::rc::Rc::new);

        let segments = self.segments.into_iter().enumerate().map(|(ix, label)| {
            let is_active = ix == selected;
            let (fg, bg) = if is_active {
                (theme.text, theme.bg_active)
            } else {
                (theme.text_faint, gpui::transparent_black())
            };
            let handler = on_select.clone();

            div()
                .id(ix)
                .flex()
                .flex_1()
                .items_center()
                .justify_center()
                .h(gpui::px(24.))
                .px_3()
                .rounded(theme.radius_sm)
                .text_size(theme.font_size)
                .text_color(fg)
                .bg(bg)
                .cursor_pointer()
                .when(!is_active, |this| {
                    this.hover(|s| s.text_color(theme.text_muted))
                })
                // Focusable so Tab reaches each segment; GPUI fires the click on
                // Enter/Space, so the focused segment becomes the selected one.
                .tab_index(0)
                .focus(move |s| s.bg(focus_bg).text_color(focus_fg))
                .child(label)
                .when_some(handler, |this, handler| {
                    this.on_click(move |_, window, cx| handler(ix, window, cx))
                })
        });

        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_0p5()
            .p_0p5()
            .rounded(theme.radius)
            .bg(theme.bg_input)
            .border_1()
            .border_color(theme.border)
            .children(segments)
    }
}
