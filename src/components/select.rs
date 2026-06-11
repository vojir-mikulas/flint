// SPDX-License-Identifier: GPL-3.0-or-later

//! A single-select dropdown. Stateless: the caller owns both the selected index
//! and the open flag, reacting via [`on_toggle`](Select::on_toggle) and
//! [`on_select`](Select::on_select). The list is deferred+anchored so it floats
//! above clipping containers.

use std::rc::Rc;

use gpui::{div, point, prelude::*, px, App, SharedString, Window};

use crate::components::floating::floating;
use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ToggleHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;
type SelectHandler = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Select {
    id: SharedString,
    options: Vec<SharedString>,
    selected: usize,
    open: bool,
    placeholder: SharedString,
    on_toggle: Option<ToggleHandler>,
    on_select: Option<SelectHandler>,
}

impl Select {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: 0,
            open: false,
            placeholder: "Select…".into(),
            on_toggle: None,
            on_select: None,
        }
    }

    pub fn option(mut self, label: impl Into<SharedString>) -> Self {
        self.options.push(label.into());
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Shown when `selected` is out of range (no selection).
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Trigger clicked, or the open list dismissed by an outside click.
    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Select {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let open = self.open;
        let selected = self.selected;
        let on_toggle = self.on_toggle.map(Rc::new);
        let on_select = self.on_select.map(Rc::new);
        let ring = theme.accent;
        let glow = theme.accent_ghost;

        let current = self
            .options
            .get(selected)
            .cloned()
            .unwrap_or_else(|| self.placeholder.clone());

        // While open, the trigger carries no click handler: dismissal is owned by
        // the list's `on_mouse_down_out`, which would otherwise immediately reopen.
        let trigger = div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .gap_2()
            .w_full()
            .h(px(32.))
            .px_2p5()
            .rounded(theme.radius)
            .bg(theme.bg_input)
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .text_sm()
            .text_color(theme.text)
            .cursor_pointer()
            .tab_index(0)
            .focus(move |s| s.focus_ring_color(ring, glow))
            .child(div().flex_1().child(current))
            .child(div().text_xs().text_color(theme.text_faint).child("⌄"))
            .when(!open, |this| {
                this.hover(|s| s.border_color(theme.border_strong))
                    .when_some(on_toggle.clone(), |this, toggle| {
                        this.on_click(move |_, window, cx| toggle(window, cx))
                    })
            });

        // One row per option, mirroring the ContextMenu item style. The selected
        // row reads in the accent color and shows a check.
        let rows = self.options.into_iter().enumerate().map(|(ix, label)| {
            let is_selected = ix == selected;
            let handler = on_select.clone();
            div()
                .id(ix)
                .flex()
                .items_center()
                .gap_2p5()
                .px_2p5()
                .py_1p5()
                .rounded(px(4.))
                .text_sm()
                .text_color(if is_selected {
                    theme.accent
                } else {
                    theme.text
                })
                .cursor_pointer()
                .tab_index(0)
                .focus(move |s| s.focus_ring_color(ring, glow))
                .hover(move |s| s.bg(theme.accent).text_color(theme.on_accent))
                .child(div().flex_1().child(label))
                .when(is_selected, |this| this.child(div().text_xs().child("✓")))
                .when_some(handler, |this, handler| {
                    this.on_click(move |_, window, cx| handler(ix, window, cx))
                })
        });

        let list = div()
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(180.))
            .p_1()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(px(7.))
            .shadow_lg()
            .when_some(on_toggle, |this, toggle| {
                this.on_mouse_down_out(move |_, window, cx| toggle(window, cx))
            })
            .children(rows);

        div().relative().w_full().child(trigger).when(open, |this| {
            // Drop the list just below the 32px trigger (+4px gap); `floating`
            // defers it above clipping containers and fits it in the window.
            this.child(floating(list).offset(point(px(0.), px(36.))))
        })
    }
}
