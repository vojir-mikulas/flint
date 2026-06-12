// SPDX-License-Identifier: GPL-3.0-or-later

//! House components, built on GPUI primitives with a consistent variant API.
//!
//! Each component lands in the gallery first (`examples/gallery.rs`), then gets
//! used by the app. More components arrive as consumers (Nyx, RED) need them.

pub mod badge;
pub mod button;
pub mod code_editor;
pub mod context_menu;
pub mod floating;
mod fuzzy;
pub mod icon_button;
pub mod modal;
pub mod number_input;
pub mod palette;
pub mod progress_bar;
pub mod scrollbar;
pub mod segmented;
pub mod select;
pub mod split_pane;
pub mod switcher;
pub mod table;
pub mod tabs;
pub mod text_input;
pub mod toast;
pub mod toggle;
pub mod tooltip;
pub mod tree;
