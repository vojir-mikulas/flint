// SPDX-License-Identifier: GPL-3.0-or-later

//! `Checkbox` - a compact square tick box for multi-select lists. Stateless: the
//! caller owns the boolean, reacting via [`on_change`](Checkbox::on_change).
//!
//! A [`Toggle`](crate::components::toggle::Toggle) reads as a settings switch
//! (on/off state that takes effect immediately); a `Checkbox` reads as row
//! selection in a list you act on later. Use this one for "pick which of these
//! to include" lists where density matters.

use gpui::{div, prelude::*, AnyElement, App, ElementId, Role, SharedString, Window};

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ChangeHandler = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    mark: Option<AnyElement>,
    on_change: Option<ChangeHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            disabled: false,
            label: None,
            mark: None,
            on_change: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Accessible name announced to assistive tech (the box has no visible text
    /// of its own). Set it to what the box selects, e.g. the row's title.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// The glyph painted inside the box when checked. Flint ships no icon assets,
    /// so the default is a plain "✓" text glyph; pass a vector tick (e.g. a
    /// masked line-icon SVG) for a crisper mark. Sized/colored by the caller —
    /// the box just centers it. Only shown in the checked state.
    pub fn mark(mut self, mark: impl IntoElement) -> Self {
        self.mark = Some(mark.into_any_element());
        self
    }

    /// Called with the toggled-to value.
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let checked = self.checked;

        // The tick only paints when checked; the empty box is just border + fill,
        // so an unticked row stays quiet. A caller-supplied `mark` wins; otherwise
        // fall back to a bold "✓" glyph (Flint carries no icon assets of its own).
        let mark = checked.then(|| {
            self.mark.unwrap_or_else(|| {
                div()
                    .text_size(gpui::px(11.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(theme.on_accent)
                    .child("✓")
                    .into_any_element()
            })
        });

        let accent = theme.accent;
        let base = div()
            .id(self.id)
            .role(Role::CheckBox)
            .when_some(self.label, |this, label| this.aria_label(label))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .size(gpui::px(16.))
            .rounded(gpui::px(4.))
            .border_1()
            .when(checked, |this| this.bg(accent).border_color(accent))
            .when(!checked, |this| {
                this.bg(theme.bg_input).border_color(theme.border)
            })
            .children(mark);

        let next = !checked;
        match (self.disabled, self.on_change) {
            // Focusable so Tab reaches it; GPUI fires the click on Enter/Space, so
            // the focused box toggles from the keyboard like a real checkbox.
            (false, Some(handler)) => base
                .cursor_pointer()
                .tab_index(0)
                .when(!checked, |this| this.hover(|s| s.border_color(accent)))
                .focus(move |s| s.border_color(accent))
                .on_click(move |_, window, cx| handler(&next, window, cx)),
            (false, None) => base.cursor_pointer(),
            (true, _) => base.disabled_look(),
        }
    }
}
