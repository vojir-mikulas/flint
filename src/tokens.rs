// SPDX-License-Identifier: GPL-3.0-or-later

//! Concrete token tables for the built-in themes, ported from
//! `design/styles.css`. Add new themes here as additional `Theme` constructors.

use gpui::{px, rgb, Hsla};

use crate::theme::Theme;

/// Hex (`0xRRGGBB`) → opaque [`Hsla`].
fn h(hex: u32) -> Hsla {
    rgb(hex).into()
}

impl Theme {
    /// Zed's **One Dark** - the primary theme.
    pub fn one_dark() -> Self {
        Theme {
            name: "One Dark".into(),

            bg_app: h(0x282c33),
            bg_panel: h(0x21242b),
            bg_panel_2: h(0x1d2026),
            bg_elevated: h(0x2f343e),
            bg_bar: h(0x24272e),
            bg_hover: h(0x2c313a),
            bg_active: h(0x323843),
            bg_selected: h(0x2e3b54),
            bg_input: h(0x1b1e24),

            border: h(0x383d48),
            border_soft: h(0x2d313a),
            border_strong: h(0x464c59),

            text: h(0xdce0e5),
            text_muted: h(0x9aa1ad),
            text_faint: h(0x6b7280),
            text_dim: h(0x565d68),

            accent: h(0x98c379),
            accent_hover: h(0xa9cf8d),
            accent_ghost: h(0x98c379).opacity(0.16),
            on_accent: h(0x1d2026),

            green: h(0x98c379),
            red: h(0xe06c75),
            blue: h(0x61afef),
            purple: h(0xc678dd),
            yellow: h(0xe5c07b),
            orange: h(0xd19a66),
            cyan: h(0x56b6c2),

            row_height: px(26.0),
            radius: px(5.0),
            radius_sm: px(3.0),
        }
    }

    /// **GitHub Dark** - the alternate theme.
    pub fn github_dark() -> Self {
        Theme {
            name: "GitHub Dark".into(),

            bg_app: h(0x0d1117),
            bg_panel: h(0x010409),
            bg_panel_2: h(0x010409),
            bg_elevated: h(0x161b22),
            bg_bar: h(0x161b22),
            bg_hover: h(0x161b22),
            bg_active: h(0x21262d),
            bg_selected: h(0x15243d),
            bg_input: h(0x010409),

            border: h(0x30363d),
            border_soft: h(0x21262d),
            border_strong: h(0x444c56),

            text: h(0xe6edf3),
            text_muted: h(0x7d8590),
            text_faint: h(0x6e7681),
            text_dim: h(0x545d68),

            accent: h(0x3fb950),
            accent_hover: h(0x56d364),
            accent_ghost: h(0x3fb950).opacity(0.18),
            on_accent: h(0x0d1117),

            green: h(0x3fb950),
            red: h(0xf85149),
            blue: h(0x79c0ff),
            purple: h(0xd2a8ff),
            yellow: h(0xe3b341),
            orange: h(0xffa657),
            cyan: h(0x76e3ea),

            row_height: px(26.0),
            radius: px(5.0),
            radius_sm: px(3.0),
        }
    }

    /// **Ayu Dark** - warm-gold accent on near-black surfaces.
    pub fn ayu_dark() -> Self {
        Theme {
            name: "Ayu Dark".into(),

            bg_app: h(0x0b0e14),
            bg_panel: h(0x0d1017),
            bg_panel_2: h(0x090c12),
            bg_elevated: h(0x11151c),
            bg_bar: h(0x0d1017),
            bg_hover: h(0x131721),
            bg_active: h(0x1a1f29),
            bg_selected: h(0x1b3a5b),
            bg_input: h(0x090c12),

            border: h(0x1d242c),
            border_soft: h(0x131721),
            border_strong: h(0x273747),

            text: h(0xbfbdb6),
            text_muted: h(0x808591),
            text_faint: h(0x565b66),
            text_dim: h(0x4d5566),

            accent: h(0xe6b450),
            accent_hover: h(0xffd173),
            accent_ghost: h(0xe6b450).opacity(0.16),
            on_accent: h(0x0b0e14),

            green: h(0xaad94c),
            red: h(0xd95757),
            blue: h(0x59c2ff),
            purple: h(0xd2a6ff),
            yellow: h(0xffd173),
            orange: h(0xff8f40),
            cyan: h(0x73d0ff),

            row_height: px(26.0),
            radius: px(5.0),
            radius_sm: px(3.0),
        }
    }

    /// **Ayu Light** - warm-gold accent on near-white surfaces.
    pub fn ayu_light() -> Self {
        Theme {
            name: "Ayu Light".into(),

            bg_app: h(0xfcfcfc),
            bg_panel: h(0xf3f4f5),
            bg_panel_2: h(0xeceef0),
            bg_elevated: h(0xffffff),
            bg_bar: h(0xf3f4f5),
            bg_hover: h(0xebedef),
            bg_active: h(0xe1e4e6),
            bg_selected: h(0xcfe3f7),
            bg_input: h(0xffffff),

            border: h(0xdcdfe2),
            border_soft: h(0xe8eaed),
            border_strong: h(0xc5c9ce),

            text: h(0x5c6166),
            text_muted: h(0x8a9199),
            text_faint: h(0xa6acb2),
            text_dim: h(0xbabfc4),

            accent: h(0xffaa33),
            accent_hover: h(0xf2940f),
            accent_ghost: h(0xffaa33).opacity(0.16),
            on_accent: h(0x422a00),

            green: h(0x86b300),
            red: h(0xe65050),
            blue: h(0x399ee6),
            purple: h(0xa37acc),
            yellow: h(0xf2ae49),
            orange: h(0xfa8d3e),
            cyan: h(0x4cbf99),

            row_height: px(26.0),
            radius: px(5.0),
            radius_sm: px(3.0),
        }
    }
}
