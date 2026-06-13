// SPDX-License-Identifier: GPL-3.0-or-later

//! The semantic theme layer: the [`Theme`] token set as a GPUI [`Global`], plus
//! the [`ActiveTheme`] accessor. Tokens are semantic and generic (`bg_panel`,
//! `accent`) - never app-specific. Concrete values live in `tokens.rs`.

use gpui::{px, App, Global, Hsla, Pixels, SharedString};

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

    /// The UI (sans) font family — chrome: toolbars, tabs, sidebars, menus,
    /// status bar. Components set this on floating surfaces (palettes, menus,
    /// tooltips) that render outside the root's inheritance tree; inline elements
    /// inherit it from the window root.
    pub font_family: SharedString,
    /// The monospace font family — code + tabular data: the editor, the result
    /// grid cells, schema identifiers. The host app drives both families; setting
    /// them to the same value makes the whole UI render in one font.
    pub mono_family: SharedString,
    /// Base ("md") UI text size. The whole type ramp ([`Theme::font_size_xs`]
    /// … [`Theme::font_size_lg`]) scales proportionally from this one value, so
    /// the host app drives the entire UI by setting just `font_size`.
    pub font_size: Pixels,
}

impl Theme {
    /// The base UI text size the built-in themes ship with. Host apps tune their
    /// hand-placed pixel sizes against this and pass them through [`Theme::scale`]
    /// so the whole UI tracks the active `font_size`.
    pub const DEFAULT_FONT_SIZE: f32 = 13.0;

    /// Scale a design-time pixel size (tuned against [`DEFAULT_FONT_SIZE`](Self::DEFAULT_FONT_SIZE))
    /// by the active base `font_size`. At the default size this is the identity,
    /// so existing layouts are pixel-stable while still tracking the user's scale.
    pub fn scale(&self, design_px: f32) -> Pixels {
        px(design_px * f32::from(self.font_size) / Self::DEFAULT_FONT_SIZE)
    }

    /// Tiny labels (deep tree rows, select-item secondary text). 9px at default.
    pub fn font_size_micro(&self) -> Pixels {
        self.scale(9.0)
    }
    /// Extra-small text (chips, shortcut hints). 11px at default.
    pub fn font_size_xs(&self) -> Pixels {
        self.scale(11.0)
    }
    /// Small / secondary text. 12px at default.
    pub fn font_size_sm(&self) -> Pixels {
        self.scale(12.0)
    }
    /// Large text (palette input, headings). 15px at default.
    pub fn font_size_lg(&self) -> Pixels {
        self.scale(15.0)
    }
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

/// Whether the user (or their OS) prefers reduced motion. When set, animated
/// components fall back to a static rendering. Stored as a [`Global`] so a
/// component can honor it without the preference being threaded through every
/// call site; defaults to `false` (animate) when the host app never sets it.
#[derive(Clone, Copy, Default)]
pub struct ReduceMotion(pub bool);

impl Global for ReduceMotion {}

/// Accessor for the [`ReduceMotion`] preference; implemented for [`App`] so
/// `cx.reduce_motion()` works in `render` whether or not the host set it.
pub trait MotionPreference {
    fn reduce_motion(&self) -> bool;
}

impl MotionPreference for App {
    fn reduce_motion(&self) -> bool {
        self.try_global::<ReduceMotion>().is_some_and(|rm| rm.0)
    }
}
