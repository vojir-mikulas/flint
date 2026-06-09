// SPDX-License-Identifier: GPL-3.0-or-later

//! The semantic theme layer: the [`Theme`] token set as a GPUI [`Global`], plus
//! the [`ActiveTheme`] accessor. Tokens are semantic and generic (`bg_panel`,
//! `accent`) - never app-specific. Concrete values live in `tokens.rs`.

use gpui::{App, Global, Hsla, Pixels};

/// A complete set of design tokens for one theme. Read in `render` via
/// `cx.theme()`. Ported from `design/styles.css`.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,

    /// Main / editor surface.
    pub bg_app: Hsla,
    /// Sidebar + bottom dock (a touch darker).
    pub bg_panel: Hsla,
    /// Deepest surface (status bar, empty states).
    pub bg_panel_2: Hsla,
    /// Modals, popovers, active tab.
    pub bg_elevated: Hsla,
    /// Toolbars / tab strip.
    pub bg_bar: Hsla,
    pub bg_hover: Hsla,
    /// Hovered/selected neutral background.
    pub bg_active: Hsla,
    /// Blue-tinted selection background.
    pub bg_selected: Hsla,
    pub bg_input: Hsla,

    pub border: Hsla,
    pub border_soft: Hsla,
    pub border_strong: Hsla,

    pub text: Hsla,
    pub text_muted: Hsla,
    pub text_faint: Hsla,
    /// Dimmest text (labels, disabled).
    pub text_dim: Hsla,

    pub accent: Hsla,
    pub accent_hover: Hsla,
    /// Translucent accent - focus-ring glow, ghost-accent surfaces.
    pub accent_ghost: Hsla,
    /// Foreground used on top of `accent`.
    pub on_accent: Hsla,

    /// Green (success, FTPS).
    pub green: Hsla,
    /// Red (error, danger).
    pub red: Hsla,
    /// Blue (info, running, FTP, folders).
    pub blue: Hsla,
    /// Purple (SFTP).
    pub purple: Hsla,
    /// Yellow (warning).
    pub yellow: Hsla,
    /// Orange (archives, secondary warning).
    pub orange: Hsla,
    /// Cyan (JSON cells, SQL operators).
    pub cyan: Hsla,

    /// File-row height.
    pub row_height: Pixels,
    pub radius: Pixels,
    /// Small corner radius (chips, icon buttons, menu items).
    pub radius_sm: Pixels,
}

impl Global for Theme {}

/// Accessor for the active [`Theme`] global; implemented for [`App`] so
/// `cx.theme()` works in `render`.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        self.global::<Theme>()
    }
}
