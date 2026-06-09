// SPDX-License-Identifier: GPL-3.0-or-later

//! `IconButton` - a compact square icon-only button. Takes its glyph as a
//! generic `impl IntoElement` child (never an icon enum), staying domain-free.

use gpui::{div, prelude::*, AnyElement, App, ClickEvent, ElementId, Window};

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Square edge length, in pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum IconButtonSize {
    Xs,
    #[default]
    Sm,
    Md,
}

impl IconButtonSize {
    fn edge(self) -> f32 {
        match self {
            IconButtonSize::Xs => 22.0,
            IconButtonSize::Sm => 24.0,
            IconButtonSize::Md => 28.0,
        }
    }
}

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: AnyElement,
    size: IconButtonSize,
    active: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: impl IntoElement) -> Self {
        Self {
            id: id.into(),
            icon: icon.into_any_element(),
            size: IconButtonSize::default(),
            active: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn size(mut self, size: IconButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Toggled-on, persistent highlight.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
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

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let edge = gpui::px(self.size.edge());
        let theme = cx.theme();

        let (fg, bg) = if self.active {
            (theme.text, theme.bg_active)
        } else {
            (theme.text_faint, gpui::transparent_black())
        };
        let hover_bg = theme.bg_hover;
        let hover_fg = theme.text_muted;
        let ring = theme.accent;
        let glow = theme.accent_ghost;

        let base = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .size(edge)
            .rounded(theme.radius_sm)
            .text_color(fg)
            .bg(bg)
            .child(self.icon);

        let interactive = if self.disabled {
            base.disabled_look()
        } else {
            base.cursor_pointer()
                .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
                .tab_index(0)
                .focus(move |s| s.focus_ring_color(ring, glow))
        };

        match (self.disabled, self.on_click) {
            (false, Some(handler)) => {
                interactive.on_click(move |event, window, cx| handler(event, window, cx))
            }
            _ => interactive,
        }
    }
}
