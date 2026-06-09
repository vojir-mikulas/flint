// SPDX-License-Identifier: GPL-3.0-or-later

//! `StyledExt` - the "@apply" layer: theme-aware style recipes that compose
//! common token combinations, so views read `div().panel(cx)`.

use gpui::{px, App, BoxShadow, Hsla, Styled};

use crate::theme::ActiveTheme;

pub trait StyledExt: Styled + Sized {
    /// Panel surface: panel background + a default border.
    fn panel(self, cx: &App) -> Self {
        self.bg(cx.theme().bg_panel)
            .border_1()
            .border_color(cx.theme().border)
    }

    /// Elevated surface (modals, popovers): elevated background + strong border.
    fn elevated(self, cx: &App) -> Self {
        self.bg(cx.theme().bg_elevated)
            .border_1()
            .border_color(cx.theme().border_strong)
    }

    /// Constrain to the standard file-row height.
    fn row_h(self, cx: &App) -> Self {
        self.h(cx.theme().row_height)
    }

    /// Accent focus ring: accent border + a soft 2px accent-ghost glow.
    fn focus_ring(self, cx: &App) -> Self {
        self.focus_ring_color(cx.theme().accent, cx.theme().accent_ghost)
    }

    /// `focus_ring` with explicit colors, for use inside a `.focus(|s| …)` style
    /// closure where the theme isn't reachable through `cx`.
    fn focus_ring_color(self, border: Hsla, glow: Hsla) -> Self {
        self.border_color(border).shadow(vec![BoxShadow {
            color: glow,
            offset: gpui::point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(2.),
            inset: false,
        }])
    }

    /// Standard disabled appearance: uniformly dimmed, non-interactive.
    fn disabled_look(self) -> Self {
        self.opacity(0.5)
    }
}

impl<T: Styled> StyledExt for T {}
