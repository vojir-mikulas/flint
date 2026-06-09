// SPDX-License-Identifier: GPL-3.0-or-later

//! `Button` - the reference component for the variant API. Stateless
//! [`RenderOnce`]; an id is required so the click handler can be attached.

use gpui::{
    div, point, prelude::*, px, AnyElement, App, BoxShadow, ClickEvent, ElementId, FocusHandle,
    Hsla, SharedString, Window,
};

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ButtonVariant {
    #[default]
    Primary,
    Secondary,
    Ghost,
    Danger,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ButtonSize {
    Sm,
    #[default]
    Md,
}

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<AnyElement>,
    variant: ButtonVariant,
    size: ButtonSize,
    disabled: bool,
    focusable: bool,
    focus_handle: Option<FocusHandle>,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant: ButtonVariant::default(),
            size: ButtonSize::default(),
            disabled: false,
            // Keyboard-focusable by default so Tab reaches buttons (e.g. modal
            // footers) and Enter/Space activate them. Opt out for chrome buttons
            // that should stay out of the tab order.
            focusable: true,
            focus_handle: None,
            on_click: None,
        }
    }

    /// Whether the button participates in keyboard focus / tab order.
    pub fn focusable(mut self, focusable: bool) -> Self {
        self.focusable = focusable;
        self
    }

    /// Drive the button's focus with a caller-owned handle (so the owner can
    /// autofocus it, e.g. a modal's primary action). Implies focusable.
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self.focusable = true;
        self
    }

    /// Leading icon - any `impl IntoElement`, never an icon enum (domain-free).
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
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

/// `(background, foreground, border, hover bg)` for a variant.
fn variant_colors(variant: ButtonVariant, theme: &crate::Theme) -> (Hsla, Hsla, Hsla, Hsla) {
    match variant {
        ButtonVariant::Primary => (
            theme.accent,
            theme.on_accent,
            theme.accent,
            theme.accent_hover,
        ),
        ButtonVariant::Secondary => (theme.bg_active, theme.text, theme.border, theme.bg_hover),
        ButtonVariant::Ghost => (
            gpui::transparent_black(),
            theme.text_muted,
            gpui::transparent_black(),
            theme.bg_hover,
        ),
        ButtonVariant::Danger => (theme.bg_active, theme.red, theme.border, theme.bg_hover),
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg, border, hover_bg) = variant_colors(self.variant, theme);

        let height = match self.size {
            ButtonSize::Sm => 24.0,
            ButtonSize::Md => 32.0,
        };

        let base = div()
            .id(self.id)
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .h(gpui::px(height))
            .px_3()
            .rounded_md()
            .text_sm()
            .bg(bg)
            .text_color(fg)
            .border_1()
            .border_color(border)
            .when_some(self.icon, |this, icon| this.child(icon))
            .child(self.label);

        let ring = theme.accent;
        let glow = theme.accent_ghost;

        let interactive = if self.disabled {
            base.disabled_look()
        } else {
            let base = base
                .cursor_pointer()
                .hover(move |s| s.bg(hover_bg).border_color(hover_bg));
            if self.focusable {
                // GPUI fires `on_click` on Enter/Space when a focusable element is
                // focused, so the tab stop alone makes the button keyboard-operable.
                let base = base.tab_index(0).focus(move |s| {
                    s.border_color(ring).shadow(vec![BoxShadow {
                        color: glow,
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(0.),
                        spread_radius: px(2.),
                        inset: false,
                    }])
                });
                match self.focus_handle {
                    Some(handle) => base.track_focus(&handle),
                    None => base,
                }
            } else {
                base
            }
        };

        match (self.disabled, self.on_click) {
            (false, Some(handler)) => {
                interactive.on_click(move |event, window, cx| handler(event, window, cx))
            }
            _ => interactive,
        }
    }
}
