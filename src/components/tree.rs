// SPDX-License-Identifier: GPL-3.0-or-later

//! `Tree` - a virtualized, fixed-row-height disclosure tree on GPUI's
//! [`uniform_list`](gpui::uniform_list). Generic and stateless like
//! [`crate::Table`]: the caller flattens its model into the currently-visible
//! [`TreeItem`]s (applying its own expansion + filtering), owns selection, and
//! renders each row's content. The tree draws only the generic chrome - per-depth
//! indent, the disclosure chevron, and the hover/selected background - and
//! reports clicks back by index into the visible list.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, uniform_list, App, ClickEvent, FocusHandle, MouseButton, Pixels, Point,
    SharedString, UniformListScrollHandle, Window,
};

use crate::theme::ActiveTheme;

/// A keyboard navigation step over a [`Tree`] — the move-selection *intent* the
/// tree emits via [`Tree::on_nav`]. The tree owns no selection or expansion
/// state; the caller moves its selection, toggles expansion, or activates a row
/// in response.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TreeNav {
    Up,
    Down,
    /// Left — collapse an expanded node, else move to the parent.
    Collapse,
    /// Right — expand a collapsed node, else descend to the first child.
    Expand,
    /// Enter — activate the selected row.
    Activate,
}

/// One visible row's structural facts - everything the chrome needs to draw it.
/// The caller builds these by flattening its model in display order; the row's
/// position in that list is the index passed back to every handler.
#[derive(Clone, Copy, Debug)]
pub struct TreeItem {
    /// Nesting level; content is indented `depth * indent` from the left.
    pub depth: usize,
    /// Whether this node can expand — draws a chevron and toggles on click.
    pub has_children: bool,
    /// Whether it's currently expanded (chevron points down).
    pub expanded: bool,
}

impl TreeItem {
    pub fn new(depth: usize, has_children: bool, expanded: bool) -> Self {
        Self {
            depth,
            has_children,
            expanded,
        }
    }

    /// A leaf row (no disclosure chevron).
    pub fn leaf(depth: usize) -> Self {
        Self::new(depth, false, false)
    }
}

type RowRenderer = Rc<dyn Fn(usize, &mut Window, &mut App) -> gpui::AnyElement + 'static>;
type SelectHandler = Rc<dyn Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static>;
type IndexHandler = Rc<dyn Fn(usize, &mut Window, &mut App) + 'static>;
/// Keyboard navigation handler — receives a [`TreeNav`] intent.
type NavHandler = Rc<dyn Fn(TreeNav, &mut Window, &mut App) + 'static>;
/// Secondary (right) click handler — the row index plus the cursor position, for
/// anchoring a context menu.
type SecondaryHandler = Rc<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static>;
/// Draws the disclosure indicator for a parent row, given its expanded state.
type DisclosureRenderer = Rc<dyn Fn(bool, &mut Window, &mut App) -> gpui::AnyElement + 'static>;

#[derive(IntoElement)]
pub struct Tree {
    id: SharedString,
    rows: Rc<Vec<TreeItem>>,
    row_height: Option<Pixels>,
    indent: Pixels,
    selected: Option<usize>,
    render_row: Option<RowRenderer>,
    on_select: Option<SelectHandler>,
    on_toggle: Option<IndexHandler>,
    on_activate: Option<IndexHandler>,
    disclosure: Option<DisclosureRenderer>,
    scroll_handle: Option<UniformListScrollHandle>,
    focus_handle: Option<FocusHandle>,
    on_nav: Option<NavHandler>,
    on_secondary: Option<SecondaryHandler>,
}

impl Tree {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            rows: Rc::new(Vec::new()),
            row_height: None,
            indent: px(14.),
            selected: None,
            render_row: None,
            on_select: None,
            on_toggle: None,
            on_activate: None,
            disclosure: None,
            scroll_handle: None,
            focus_handle: None,
            on_nav: None,
            on_secondary: None,
        }
    }

    /// The currently-visible rows, in display order (caller-flattened).
    pub fn rows(mut self, rows: Vec<TreeItem>) -> Self {
        self.rows = Rc::new(rows);
        self
    }

    /// Defaults to the theme's `row_height`.
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }

    /// Horizontal indent applied per nesting level. Defaults to 14px.
    pub fn indent(mut self, indent: Pixels) -> Self {
        self.indent = indent;
        self
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// Bind the list's scroll position to a caller-owned handle.
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// Builds the content right of the chevron for a row (icon · label · badges).
    /// Stays domain-free: the caller renders whatever its model needs.
    pub fn render_row(
        mut self,
        renderer: impl Fn(usize, &mut Window, &mut App) -> gpui::AnyElement + 'static,
    ) -> Self {
        self.render_row = Some(Rc::new(renderer));
        self
    }

    /// Override the disclosure indicator drawn in the chevron slot of parent
    /// rows. The renderer receives the row's `expanded` state and returns the
    /// element (e.g. an icon). Leaf rows never call it. Defaults to a small
    /// `▶`/`▼` text glyph when unset.
    pub fn disclosure(
        mut self,
        renderer: impl Fn(bool, &mut Window, &mut App) -> gpui::AnyElement + 'static,
    ) -> Self {
        self.disclosure = Some(Rc::new(renderer));
        self
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(handler));
        self
    }

    /// Disclosure toggle — fired (in addition to [`on_select`](Self::on_select))
    /// on a single click of a row with children.
    pub fn on_toggle(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Rc::new(handler));
        self
    }

    /// Double-click; does not also fire [`on_select`](Self::on_select).
    pub fn on_activate(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }

    /// Make the tree keyboard-focusable via a caller-owned handle, so its
    /// selection can be driven by the keyboard. Pair with [`on_nav`](Self::on_nav).
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self
    }

    /// Keyboard navigation handler — ↑/↓ move, ←/→ collapse/expand, Enter
    /// activates, each emitted as a [`TreeNav`] intent. Requires a
    /// [`focus_handle`](Self::focus_handle).
    pub fn on_nav(mut self, handler: impl Fn(TreeNav, &mut Window, &mut App) + 'static) -> Self {
        self.on_nav = Some(Rc::new(handler));
        self
    }

    /// Secondary (right-click) handler — fired with the row index and the cursor
    /// position, for opening a context menu. Independent of select/toggle/activate.
    pub fn on_secondary(
        mut self,
        handler: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Tree {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let row_height = self.row_height.unwrap_or(theme.row_height);
        let indent = self.indent;
        let rows = self.rows.clone();
        let row_count = rows.len();

        let render_row = self.render_row.clone();
        let disclosure = self.disclosure.clone();
        let on_select = self.on_select.clone();
        let on_toggle = self.on_toggle.clone();
        let on_activate = self.on_activate.clone();
        let on_secondary = self.on_secondary.clone();
        let selected = self.selected;

        // Token snapshot so the `'static` row closure doesn't borrow `cx`.
        let bg_hover = theme.bg_hover;
        let bg_selected = theme.bg_selected;
        let accent = theme.accent;
        let chevron_color = theme.text_faint;
        let text = theme.text;
        let radius = theme.radius_sm;
        let glyph_size = theme.font_size_micro();

        let list = uniform_list("tree-rows", row_count, move |range, window, cx| {
            let mut out = Vec::with_capacity(range.len());
            for ix in range {
                let item = rows[ix];
                let is_selected = selected == Some(ix);

                let content = render_row
                    .as_ref()
                    .map(|r| r(ix, window, cx))
                    .unwrap_or_else(|| div().into_any_element());

                // Chevron slot: a disclosure indicator for parents, else an empty
                // spacer so labels line up across leaf and parent rows. A custom
                // `disclosure` renderer wins; otherwise a small text glyph.
                let custom_disclosure = if item.has_children {
                    disclosure.as_ref().map(|r| r(item.expanded, window, cx))
                } else {
                    None
                };
                let glyph_disclosure = item.has_children && disclosure.is_none();
                let chevron = div()
                    .w(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(chevron_color)
                    .when_some(custom_disclosure, |d, el| d.child(el))
                    .when(glyph_disclosure, |d| {
                        d.text_size(glyph_size)
                            .child(if item.expanded { "▼" } else { "▶" })
                    });

                let on_select = on_select.clone();
                let on_toggle = on_toggle.clone();
                let on_activate = on_activate.clone();
                let on_secondary = on_secondary.clone();
                let has_children = item.has_children;
                let clickable = on_select.is_some() || on_toggle.is_some() || on_activate.is_some();

                out.push(
                    div()
                        .id(ix)
                        .flex()
                        .items_center()
                        .w_full()
                        .h(row_height)
                        .pr(px(8.))
                        // Base inset + per-depth indent. A 2px left border always
                        // reserves space (transparent unless selected) so toggling
                        // selection never nudges the row horizontally.
                        .pl(px(4.) + indent * item.depth as f32)
                        .border_l_2()
                        .border_color(if is_selected {
                            accent
                        } else {
                            gpui::transparent_black()
                        })
                        .rounded(radius)
                        .text_color(text)
                        .when(is_selected, |d| d.bg(bg_selected))
                        .when(!is_selected, |d| d.hover(move |s| s.bg(bg_hover)))
                        .when(clickable, |d| d.cursor_pointer())
                        .when(clickable, |d| {
                            d.on_click(move |event, window, cx| {
                                if event.click_count() >= 2 {
                                    if let Some(on_activate) = on_activate.as_ref() {
                                        on_activate(ix, window, cx);
                                        return;
                                    }
                                }
                                if let Some(on_select) = on_select.as_ref() {
                                    on_select(ix, event, window, cx);
                                }
                                if has_children {
                                    if let Some(on_toggle) = on_toggle.as_ref() {
                                        on_toggle(ix, window, cx);
                                    }
                                }
                            })
                        })
                        .when(on_secondary.is_some(), |d| {
                            d.cursor_pointer().on_mouse_down(
                                MouseButton::Right,
                                move |event, window, cx| {
                                    if let Some(on_secondary) = on_secondary.as_ref() {
                                        on_secondary(ix, event.position, window, cx);
                                    }
                                },
                            )
                        })
                        .child(chevron)
                        .child(content),
                );
            }
            out
        })
        .flex_1();

        let list = match self.scroll_handle.as_ref() {
            Some(handle) => list.track_scroll(handle),
            None => list,
        };

        let on_nav = self.on_nav.clone();
        div()
            .id(self.id)
            .flex()
            .flex_col()
            .size_full()
            .child(list)
            .when_some(self.focus_handle, |d, handle| {
                let d = d.track_focus(&handle).key_context("Tree");
                match on_nav {
                    Some(on_nav) => d.on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
                        let nav = match event.keystroke.key.as_str() {
                            "up" => TreeNav::Up,
                            "down" => TreeNav::Down,
                            "left" => TreeNav::Collapse,
                            "right" => TreeNav::Expand,
                            "enter" => TreeNav::Activate,
                            _ => return,
                        };
                        cx.stop_propagation();
                        on_nav(nav, window, cx);
                    }),
                    None => d,
                }
            })
    }
}
