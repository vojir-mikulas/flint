// SPDX-License-Identifier: GPL-3.0-or-later

//! `Tabs` - a horizontal strip of selectable tabs with an optional count pill.
//! Stateless: the caller owns the selected index, reacting via
//! [`on_select`](Tabs::on_select).

use gpui::{div, prelude::*, App, Role, SharedString, Window};

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type SelectHandler = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

struct TabItem {
    label: SharedString,
    count: Option<usize>,
}

#[derive(IntoElement)]
pub struct Tabs {
    id: SharedString,
    items: Vec<TabItem>,
    selected: usize,
    on_select: Option<SelectHandler>,
}

impl Tabs {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            selected: 0,
            on_select: None,
        }
    }

    pub fn tab(mut self, label: impl Into<SharedString>, count: Option<usize>) -> Self {
        self.items.push(TabItem {
            label: label.into(),
            count,
        });
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

impl RenderOnce for Tabs {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let selected = self.selected;
        let on_select = self.on_select.map(std::rc::Rc::new);
        let ring = theme.accent;
        let glow = theme.accent_ghost;

        let tabs = self.items.into_iter().enumerate().map(|(ix, item)| {
            let is_active = ix == selected;
            let (fg, bg) = if is_active {
                (theme.text, theme.bg_active)
            } else {
                (theme.text_faint, gpui::transparent_black())
            };
            let handler = on_select.clone();

            let pill = item.count.map(|count| {
                let (pill_fg, pill_bg) = if is_active {
                    (theme.on_accent, theme.accent)
                } else {
                    (theme.text_muted, theme.bg_elevated)
                };
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .min_w(gpui::px(16.))
                    .h(gpui::px(16.))
                    .pl(gpui::px(5.))
                    .pr(gpui::px(3.))
                    .rounded_full()
                    .text_size(gpui::px(10.))
                    .line_height(gpui::px(16.))
                    .text_color(pill_fg)
                    .bg(pill_bg)
                    .child(count.to_string())
            });

            div()
                .id(ix)
                .role(Role::Tab)
                .aria_label(item.label.clone())
                .aria_selected(is_active)
                .flex()
                .items_center()
                .gap_1p5()
                .h(gpui::px(24.))
                .px_2p5()
                .rounded(theme.radius_sm)
                .text_size(theme.font_size)
                .text_color(fg)
                .bg(bg)
                .cursor_pointer()
                .tab_index(0)
                .focus(move |s| s.focus_ring_color(ring, glow))
                .when(!is_active, |this| {
                    this.hover(|s| s.bg(theme.bg_hover).text_color(theme.text_muted))
                })
                .child(item.label)
                .when_some(pill, |this, pill| this.child(pill))
                .when_some(handler, |this, handler| {
                    this.on_click(move |_, window, cx| handler(ix, window, cx))
                })
        });

        div()
            .id(self.id)
            .role(Role::TabList)
            .flex()
            .items_center()
            .gap_0p5()
            .children(tabs)
    }
}
