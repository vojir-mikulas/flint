// SPDX-License-Identifier: GPL-3.0-or-later

//! `IconButton` - a compact square icon-only button. Takes its glyph as a
//! generic `impl IntoElement` child (never an icon enum), staying domain-free.

use gpui::{div, prelude::*, AnyElement, App, ClickEvent, ElementId, Role, SharedString, Window};

use crate::components::tooltip::Tooltip;
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
    tooltip: Option<SharedString>,
    a11y_label: Option<SharedString>,
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
            tooltip: None,
            a11y_label: None,
        }
    }

    /// A hover tooltip (e.g. the action's name + its keyboard shortcut).
    ///
    /// The tooltip text also seeds the accessible name when no explicit
    /// [`a11y_label`](Self::a11y_label) is set — an icon-only button is opaque
    /// to assistive technology otherwise, since a tooltip is hover-only and
    /// never exposed to AT. Prefer a tooltip on every icon button.
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    /// The accessible name reported to assistive technology. Set this when the
    /// tooltip carries a keyboard hint you don't want spoken (e.g. tooltip
    /// "Settings  ⌘," → a11y_label "Settings"), or when there is no tooltip.
    pub fn a11y_label(mut self, label: impl Into<SharedString>) -> Self {
        self.a11y_label = Some(label.into());
        self
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

        // Accessible name: explicit override, else the tooltip text. Without one
        // an icon-only button is an unnamed control to a screen reader.
        let a11y_name = self.a11y_label.clone().or_else(|| self.tooltip.clone());
        let base = div()
            .id(self.id)
            .role(Role::Button)
            .when_some(a11y_name, |d, name| d.aria_label(name))
            .when(self.active, |d| d.aria_selected(true))
            .flex()
            .items_center()
            .justify_center()
            .size(edge)
            .rounded(theme.radius_sm)
            .text_color(fg)
            .bg(bg)
            .child(self.icon)
            .when_some(self.tooltip, |d, text| d.tooltip(Tooltip::text(text)));

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
