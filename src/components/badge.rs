// SPDX-License-Identifier: GPL-3.0-or-later

//! `Badge` - a small colored label chip. Variants are semantic; the fill is the
//! text color at low opacity, so it tracks the active theme.

use gpui::{div, prelude::*, App, Hsla, SharedString, Window};

use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BadgeVariant {
    #[default]
    Neutral,
    Accent,
    Success,
    Danger,
    Info,
    Warning,
    Special,
}

#[derive(IntoElement)]
pub struct Badge {
    label: SharedString,
    variant: BadgeVariant,
}

impl Badge {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            variant: BadgeVariant::default(),
        }
    }

    pub fn variant(mut self, variant: BadgeVariant) -> Self {
        self.variant = variant;
        self
    }
}

fn variant_colors(variant: BadgeVariant, theme: &crate::Theme) -> (Hsla, Hsla) {
    let tinted = |c: Hsla| (c, c.opacity(0.13));
    match variant {
        BadgeVariant::Neutral => (theme.text_muted, theme.bg_active),
        BadgeVariant::Accent => tinted(theme.accent),
        BadgeVariant::Success => tinted(theme.green),
        BadgeVariant::Danger => tinted(theme.red),
        BadgeVariant::Info => tinted(theme.blue),
        BadgeVariant::Warning => tinted(theme.yellow),
        BadgeVariant::Special => tinted(theme.purple),
    }
}

impl RenderOnce for Badge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (fg, bg) = variant_colors(self.variant, theme);

        div()
            .flex()
            .items_center()
            .px_1p5()
            .py(gpui::px(1.))
            .rounded(theme.radius_sm)
            .text_size(theme.font_size_xs())
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(fg)
            .bg(bg)
            .child(self.label)
    }
}
