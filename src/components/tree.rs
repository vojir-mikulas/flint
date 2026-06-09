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
    div, prelude::*, px, uniform_list, App, ClickEvent, Pixels, SharedString,
    UniformListScrollHandle, Window,
};

use crate::theme::ActiveTheme;

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
    scroll_handle: Option<UniformListScrollHandle>,
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
            scroll_handle: None,
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
}

impl RenderOnce for Tree {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let row_height = self.row_height.unwrap_or(theme.row_height);
        let indent = self.indent;
        let rows = self.rows.clone();
        let row_count = rows.len();

        let render_row = self.render_row.clone();
        let on_select = self.on_select.clone();
        let on_toggle = self.on_toggle.clone();
        let on_activate = self.on_activate.clone();
        let selected = self.selected;

        // Token snapshot so the `'static` row closure doesn't borrow `cx`.
        let bg_hover = theme.bg_hover;
        let bg_selected = theme.bg_selected;
        let accent = theme.accent;
        let chevron_color = theme.text_faint;
        let text = theme.text;
        let radius = theme.radius_sm;

        let list = uniform_list("tree-rows", row_count, move |range, window, cx| {
            let mut out = Vec::with_capacity(range.len());
            for ix in range {
                let item = rows[ix];
                let is_selected = selected == Some(ix);

                let content = render_row
                    .as_ref()
                    .map(|r| r(ix, window, cx))
                    .unwrap_or_else(|| div().into_any_element());

                // Chevron slot: a disclosure triangle for parents, else an empty
                // spacer so labels line up across leaf and parent rows.
                let chevron = div()
                    .w(px(16.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(chevron_color)
                    .when(item.has_children, |d| {
                        d.text_size(px(9.))
                            .child(if item.expanded { "▼" } else { "▶" })
                    });

                let on_select = on_select.clone();
                let on_toggle = on_toggle.clone();
                let on_activate = on_activate.clone();
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

        div().id(self.id).flex().flex_col().size_full().child(list)
    }
}
