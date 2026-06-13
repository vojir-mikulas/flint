// SPDX-License-Identifier: GPL-3.0-or-later

//! `Toggle` - a binary on/off switch. Stateless: the caller owns the boolean,
//! reacting via [`on_change`](Toggle::on_change).

use gpui::{div, prelude::*, App, ElementId, Role, SharedString, Window};

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ChangeHandler = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    checked: bool,
    disabled: bool,
    label: Option<SharedString>,
    on_change: Option<ChangeHandler>,
}

impl Toggle {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Self {
            id: id.into(),
            checked,
            disabled: false,
            label: None,
            on_change: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Accessible name announced to assistive tech (the switch has no visible
    /// text of its own). Set it to what the switch controls, e.g. "Read-only".
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Called with the toggled-to value.
    pub fn on_change(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let checked = self.checked;
        let track_bg = if checked {
            theme.accent
        } else {
            theme.bg_active
        };
        let knob = div()
            .size(gpui::px(14.))
            .flex_shrink_0()
            .mx(gpui::px(2.))
            .rounded_full()
            .bg(if checked {
                theme.on_accent
            } else {
                theme.text_muted
            });

        let accent = theme.accent;
        let base = div()
            .id(self.id)
            // A switch role with its accessible name, so screen readers announce
            // it and its checked state rather than an anonymous box.
            .role(Role::CheckBox)
            .when_some(self.label, |this, label| this.aria_label(label))
            .flex()
            .items_center()
            .w(gpui::px(34.))
            .h(gpui::px(18.))
            .rounded_full()
            .bg(track_bg)
            .border_1()
            .border_color(if checked { theme.accent } else { theme.border })
            .when(checked, |this| this.justify_end())
            .when(!checked, |this| this.justify_start())
            .child(knob);

        let next = !checked;
        match (self.disabled, self.on_change) {
            // Focusable so Tab reaches it; GPUI fires the click on Enter/Space, so
            // the focused switch toggles from the keyboard like a real checkbox.
            (false, Some(handler)) => base
                .cursor_pointer()
                .tab_index(0)
                .focus(move |s| s.border_color(accent))
                .on_click(move |_, window, cx| handler(&next, window, cx)),
            (false, None) => base.cursor_pointer(),
            (true, _) => base.disabled_look(),
        }
    }
}
