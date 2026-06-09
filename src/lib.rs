// SPDX-License-Identifier: GPL-3.0-or-later

//! **Flint** — an in-house GPUI component library and semantic theme layer.
//!
//! Hard rule: this crate **must never depend on any application or domain
//! crate**. It builds on `gpui` alone, so it can be shared verbatim by any GPUI
//! app (Nyx, RED, …). Map domain types to generic props *in the app*.

pub mod components;
pub mod styled_ext;
pub mod theme;

mod tokens;

// Re-export the pinned GPUI so consumers use `flint::gpui` instead of declaring
// their own `gpui` dep. Cargo unifies two git deps only at the *same* rev;
// routing every consumer through this one re-export makes a second, incompatible
// gpui copy impossible. Keystone of the cross-repo GPUI-rev contract.
pub use gpui;

pub use components::badge::{Badge, BadgeVariant};
pub use components::button::{Button, ButtonSize, ButtonVariant};
pub use components::code_editor::{
    CodeEditor, CodeEditorEvent, CompletionProvider, Highlighter, TokenStyle,
};
pub use components::context_menu::{ContextMenu, ContextMenuItem};
pub use components::icon_button::{IconButton, IconButtonSize};
pub use components::modal::Modal;
pub use components::progress_bar::ProgressBar;
pub use components::segmented::Segmented;
pub use components::select::Select;
pub use components::split_pane::{DragAnchor, SplitPane};
pub use components::table::{Column, ColumnAlign, ColumnWidth, Table};
pub use components::tabs::Tabs;
pub use components::text_input::{TextInput, TextInputEvent};
pub use components::toast::{Toast, ToastVariant};
pub use components::toggle::Toggle;
pub use components::tooltip::Tooltip;
pub use components::tree::{Tree, TreeItem};
pub use styled_ext::StyledExt;
pub use theme::{ActiveTheme, Theme};

/// Everything you need with a single `use flint::prelude::*;`.
pub mod prelude {
    pub use crate::components::badge::{Badge, BadgeVariant};
    pub use crate::components::button::{Button, ButtonSize, ButtonVariant};
    pub use crate::components::code_editor::{
        CodeEditor, CodeEditorEvent, CompletionProvider, Highlighter, TokenStyle,
    };
    pub use crate::components::context_menu::{ContextMenu, ContextMenuItem};
    pub use crate::components::icon_button::{IconButton, IconButtonSize};
    pub use crate::components::modal::Modal;
    pub use crate::components::progress_bar::ProgressBar;
    pub use crate::components::segmented::Segmented;
    pub use crate::components::select::Select;
    pub use crate::components::split_pane::{DragAnchor, SplitPane};
    pub use crate::components::table::{Column, ColumnAlign, ColumnWidth, Table};
    pub use crate::components::tabs::Tabs;
    pub use crate::components::text_input::{TextInput, TextInputEvent};
    pub use crate::components::toast::{Toast, ToastVariant};
    pub use crate::components::toggle::Toggle;
    pub use crate::components::tooltip::Tooltip;
    pub use crate::components::tree::{Tree, TreeItem};
    pub use crate::styled_ext::StyledExt;
    pub use crate::theme::{ActiveTheme, Theme};
}
