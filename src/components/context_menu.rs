// SPDX-License-Identifier: GPL-3.0-or-later

//! `ContextMenu` - a floating list of actions. Renders only the menu surface;
//! the caller anchors it (via [`gpui::anchored`] / [`gpui::deferred`]).
//!
//! Long menus stay inside the window: the surface caps its height to the
//! viewport and scrolls internally rather than spilling off the top/bottom edge.
//! An entry may be a [`Submenu`] — a nested surface that flies out to the side
//! when the caller marks it open. The open flag and the hover callback are owned
//! by the caller (mirroring [`crate::components::Select`]), so `ContextMenu`
//! itself stays a stateless `RenderOnce`.

use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, App, Bounds, ClickEvent, Div, Entity, Pixels,
    SharedString, Stateful, Window,
};

use crate::components::floating::floating;
use crate::styled_ext::StyledExt;
use crate::theme::{ActiveTheme, Theme};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Which submenu (by key) is currently flown out. Owned internally by
/// [`ContextMenu`] via `use_keyed_state`, so it resets when the menu closes.
type OpenSubmenu = Entity<Option<SharedString>>;

pub struct ContextMenuItem {
    key: SharedString,
    label: SharedString,
    shortcut: Option<SharedString>,
    danger: bool,
    disabled: bool,
    on_click: Option<ClickHandler>,
}

impl ContextMenuItem {
    /// `key` doubles as the element id.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            shortcut: None,
            danger: false,
            disabled: false,
            on_click: None,
        }
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

/// A nested menu that flies out from a parent row. [`ContextMenu`] tracks which
/// submenu is open itself (keyed on this `key`): entering the parent row opens
/// the flyout, and entering any *sibling* row closes it again — no caller state
/// required. The flyout is anchored to the parent row's measured right edge and
/// is deferred, so it escapes the menu's scroll clip.
pub struct Submenu {
    key: SharedString,
    label: SharedString,
    items: Vec<ContextMenuItem>,
}

impl Submenu {
    /// `key` doubles as the element id and the anchor-state key.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            items: Vec::new(),
        }
    }

    pub fn item(mut self, item: ContextMenuItem) -> Self {
        self.items.push(item);
        self
    }
}

enum Entry {
    Item(ContextMenuItem),
    Separator,
    Submenu(Submenu),
}

#[derive(IntoElement)]
pub struct ContextMenu {
    id: SharedString,
    entries: Vec<Entry>,
}

impl ContextMenu {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            entries: Vec::new(),
        }
    }

    pub fn item(mut self, item: ContextMenuItem) -> Self {
        self.entries.push(Entry::Item(item));
        self
    }

    pub fn separator(mut self) -> Self {
        self.entries.push(Entry::Separator);
        self
    }

    pub fn submenu(mut self, submenu: Submenu) -> Self {
        self.entries.push(Entry::Submenu(submenu));
        self
    }
}

impl RenderOnce for ContextMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let menu_id = self.id;
        // Cap to the viewport (less a small edge margin) so a long menu — every
        // FK column of a wide table, say — scrolls inside the window instead of
        // running off its top or bottom.
        let max_h = (window.viewport_size().height - px(16.)).max(px(120.));

        // Which submenu is flown out, tracked internally. `use_keyed_state` is
        // retained only while the menu is on screen, so this resets to `None`
        // whenever the menu closes — no stale flyout on the next open.
        let open_state: OpenSubmenu = window.use_keyed_state(
            SharedString::from(format!("{menu_id}__open_sub")),
            cx,
            |_, _| None,
        );
        let open_key = open_state.read(cx).clone();

        // Rows are built imperatively because a submenu needs `&mut Window`/`&mut
        // App` (to measure its anchor) — which a `map` closure can't thread.
        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            rows.push(match entry {
                Entry::Separator => separator_el(&theme),
                // A top-level row hovering closes any open flyout (`hover_open` =
                // None); a submenu opens its own.
                Entry::Item(item) => item_el(item, open_state.clone(), None, &theme),
                Entry::Submenu(sub) => {
                    submenu_el(sub, open_state.clone(), open_key.clone(), max_h, &theme, window, cx)
                }
            });
        }

        surface(menu_id, max_h, &theme).children(rows)
    }
}

/// The shared menu chrome: an id'd, viewport-capped, scrollable column.
fn surface(id: SharedString, max_h: Pixels, theme: &Theme) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .min_w(px(180.))
        .max_h(max_h)
        .overflow_y_scroll()
        .p_1()
        .font_family(theme.font_family.clone())
        .text_size(theme.font_size)
        .bg(theme.bg_elevated)
        .border_1()
        .border_color(theme.border)
        .rounded(px(7.))
        .shadow_lg()
}

fn separator_el(theme: &Theme) -> AnyElement {
    div()
        .h(px(1.))
        .mx(px(2.))
        .my(px(4.))
        .bg(theme.border_soft)
        .into_any_element()
}

/// A single clickable (or disabled) menu row.
///
/// `hover_open` is the value written to the shared open-submenu state when this
/// row is entered: `None` for a top-level row (so hovering it collapses any open
/// flyout), or `Some(parent_key)` for a row *inside* a flyout (so hovering its
/// own items keeps the flyout up). This is what makes an open submenu close when
/// the cursor moves to a neighbouring item.
fn item_el(
    item: ContextMenuItem,
    open_state: OpenSubmenu,
    hover_open: Option<SharedString>,
    theme: &Theme,
) -> AnyElement {
    let base_color = if item.danger { theme.red } else { theme.text };
    let hover_bg = if item.danger { theme.red } else { theme.accent };
    let hover_fg = theme.on_accent;
    let row = div()
        .id(item.key)
        .flex()
        .items_center()
        .gap_2p5()
        .px_2p5()
        .py_1p5()
        .rounded(px(4.))
        .text_color(base_color)
        .child(div().flex_1().text_size(theme.font_size).child(item.label))
        .when_some(item.shortcut, |this, sc| {
            this.child(
                div()
                    .text_size(theme.font_size_xs())
                    .text_color(theme.text_faint)
                    .child(sc),
            )
        })
        .on_hover(move |hovered, _, cx| {
            if *hovered {
                open_state.update(cx, |cur, cx| {
                    if *cur != hover_open {
                        cur.clone_from(&hover_open);
                        cx.notify();
                    }
                });
            }
        });

    if item.disabled {
        row.disabled_look().into_any_element()
    } else {
        row.cursor_pointer()
            .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
            .when_some(item.on_click, |this, handler| {
                this.on_click(move |event, window, cx| handler(event, window, cx))
            })
            .into_any_element()
    }
}

/// A parent row that flies its children out to the right. The row records its
/// own window bounds via a `canvas` so the flyout can anchor to the row's
/// right edge — a stateless `anchored()` relative guess can't account for the
/// menu's content width or scroll offset.
fn submenu_el(
    sub: Submenu,
    open_state: OpenSubmenu,
    open_key: Option<SharedString>,
    max_h: Pixels,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let key = sub.key;
    let is_open = open_key.as_ref() == Some(&key);

    let bounds_state = window.use_keyed_state(
        SharedString::from(format!("{key}__sub_bounds")),
        cx,
        |_, _| None::<Bounds<Pixels>>,
    );
    let anchor = *bounds_state.read(cx);
    let measure = bounds_state.clone();

    let hover_bg = theme.accent;
    let hover_fg = theme.on_accent;

    let mut row = div()
        .id(key.clone())
        .relative()
        .flex()
        .items_center()
        .gap_2p5()
        .px_2p5()
        .py_1p5()
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(theme.text)
        .child(div().flex_1().text_size(theme.font_size).child(sub.label))
        .child(
            // Trailing disclosure chevron marks the row as a flyout.
            div()
                .text_size(theme.font_size_xs())
                .text_color(theme.text_faint)
                .child("›"),
        )
        .child(
            // Invisible overlay that records the row's window bounds each frame.
            canvas(
                move |bounds, _, cx| {
                    measure.update(cx, |stored, cx| {
                        if *stored != Some(bounds) {
                            *stored = Some(bounds);
                            cx.notify();
                        }
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );

    if is_open {
        // Keep the row lit while its flyout stands open.
        row = row.bg(hover_bg).text_color(hover_fg);
    } else {
        row = row.hover(move |s| s.bg(hover_bg).text_color(hover_fg));
    }
    // Entering the parent row opens this submenu (and, via the shared state,
    // closes any other that was open).
    let hover_state = open_state.clone();
    let hover_key = key.clone();
    row = row.on_hover(move |hovered, _, cx| {
        if *hovered {
            hover_state.update(cx, |cur, cx| {
                if cur.as_ref() != Some(&hover_key) {
                    *cur = Some(hover_key.clone());
                    cx.notify();
                }
            });
        }
    });

    let items = sub.items;
    let sub_id = SharedString::from(format!("{key}__flyout"));
    let flyout_theme = theme.clone();
    // Flyout rows report `Some(key)` on hover so moving the cursor across them
    // keeps this submenu open rather than collapsing it.
    let flyout_open = Some(key);
    // Stack as a full-width column so the parent row fills the menu like every
    // other item (a plain flex-row wrapper would shrink it to content width).
    // That also puts the row's measured right edge at the menu edge, so the
    // flyout anchors beside the menu instead of overlapping it.
    div()
        .flex()
        .flex_col()
        .child(row)
        .when(is_open, move |this| match anchor {
            Some(b) => {
                let rows = items
                    .into_iter()
                    .map(|it| item_el(it, open_state.clone(), flyout_open.clone(), &flyout_theme))
                    .collect::<Vec<_>>();
                let flyout = surface(sub_id, max_h, &flyout_theme)
                    .occlude()
                    .children(rows);
                // Anchor to the row's measured top-right; the small nudge lines
                // the flyout's first item up with the parent (offsets the 4px
                // surface padding) and leaves a 2px gap from the row edge.
                this.child(
                    floating(flyout)
                        .at(b.top_right())
                        .offset(point(px(2.), px(-5.))),
                )
            }
            None => this,
        })
        .into_any_element()
}
