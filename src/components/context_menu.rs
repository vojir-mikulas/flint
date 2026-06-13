// SPDX-License-Identifier: GPL-3.0-or-later

//! `ContextMenu` - a floating list of actions. Renders only the menu surface;
//! the caller anchors it (via [`gpui::anchored`] / [`gpui::deferred`]).

use gpui::{div, prelude::*, App, ClickEvent, SharedString, Window};

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

pub struct ContextMenuItem {
    key: SharedString,
    label: SharedString,
    shortcut: Option<SharedString>,
    danger: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl ContextMenuItem {
    /// `key` doubles as the element id.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            shortcut: None,
            danger: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

enum Entry {
    Item(ContextMenuItem),
    Separator,
}

#[derive(IntoElement)]
pub struct ContextMenu {
    id: SharedString,
    entries: Vec<Entry>,
}

impl ContextMenu {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            entries: Vec::new(),
        }
    }

    pub fn item(mut self, item: ContextMenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }

    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let menu_id = self.id;

        let rows = self.entries.into_iter().map(|entry| match entry {
            Entry::Separator => div()
                .h(gpui::px(1.))
                .mx(gpui::px(2.))
                .my(gpui::px(4.))
                .bg(theme.border_soft)
                .into_any_element(),
            Entry::Item(item) => {
                let base_color = if item.danger { theme.red } else { theme.text };
                let hover_bg = if item.danger { theme.red } else { theme.accent };
                let hover_fg = theme.on_accent;
                let row = div()
                    .id(item.key.clone())
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px_2p5()
                    .py_1p5()
                    .rounded(gpui::px(4.))
                    .text_color(base_color)
                    .child(div().flex_1().text_size(theme.font_size).child(item.label))
                    .when_some(item.shortcut, |this, sc| {
                        this.child(
                            div()
                                .text_size(theme.font_size_xs())
                                .text_color(theme.text_faint)
                                .child(sc),
                        )
                    });

                if item.disabled {
                    row.disabled_look().into_any_element()
                } else {
                    row.cursor_pointer()
                        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                        .when_some(item.on_click, |this, handler| {
                            this.on_click(move |event, window, cx| handler(event, window, cx))
                        })
                        .into_any_element()
                }
            }
        });

        div()
            .id(menu_id)
            .flex()
            .flex_col()
            .min_w(gpui::px(180.))
            .p_1()
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size)
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .rounded(gpui::px(7.))
            .shadow_lg()
            .children(rows)
    }
}
