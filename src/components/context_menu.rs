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
    canvas, div, point, prelude::*, px, AnyElement, App, Bounds, ClickEvent, Div, Pixels,
    SharedString, Stateful, Window,
};

use crate::components::floating::floating;
use crate::styled_ext::StyledExt;
use crate::theme::{ActiveTheme, Theme};

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
type HoverHandler = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

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

/// A nested menu that flies out from a parent row. The caller controls
/// [`open`](Self::open) and [`on_hover`](Self::on_hover) so [`ContextMenu`] can
/// stay stateless: the typical wiring is `on_hover` → set a `bool` on the owning
/// view → feed it back as `open`. The flyout is anchored to the parent row's
/// measured right edge and is deferred, so it escapes the menu's scroll clip.
pub struct Submenu {
    key: SharedString,
    label: SharedString,
    open: bool,
    on_hover: Option<HoverHandler>,
    items: Vec<ContextMenuItem>,
}

impl Submenu {
    /// `key` doubles as the element id and the anchor-state key.
    pub fn new(key: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            open: false,
            on_hover: None,
            items: Vec::new(),
        }
    }

    /// Whether the flyout is shown. Owned by the caller's view state.
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Fired with `true` when the parent row is entered, `false` when left.
    pub fn on_hover(mut self, handler: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Box::new(handler));
        self
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

        // Rows are built imperatively because a submenu needs `&mut Window`/`&mut
        // App` (to measure its anchor) — which a `map` closure can't thread.
        let mut rows: Vec<AnyElement> = Vec::with_capacity(self.entries.len());
        for entry in self.entries {
            rows.push(match entry {
                Entry::Separator => separator_el(&theme),
                Entry::Item(item) => item_el(item, &theme),
                Entry::Submenu(sub) => submenu_el(sub, max_h, &theme, window, cx),
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
fn item_el(item: ContextMenuItem, theme: &Theme) -> AnyElement {
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
    max_h: Pixels,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let bounds_state = window.use_keyed_state(
        SharedString::from(format!("{}__sub_bounds", sub.key)),
        cx,
        |_, _| None::<Bounds<Pixels>>,
    );
    let anchor = *bounds_state.read(cx);
    let measure = bounds_state.clone();

    let hover_bg = theme.accent;
    let hover_fg = theme.on_accent;

    let mut row = div()
        .id(sub.key.clone())
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

    if sub.open {
        // Keep the row lit while its flyout stands open.
        row = row.bg(hover_bg).text_color(hover_fg);
    } else {
        row = row.hover(move |s| s.bg(hover_bg).text_color(hover_fg));
    }
    if let Some(handler) = sub.on_hover {
        row = row.on_hover(move |hovered, window, cx| handler(hovered, window, cx));
    }

    let open = sub.open;
    let items = sub.items;
    let sub_id = SharedString::from(format!("{}__flyout", sub.key));
    let flyout_theme = theme.clone();
    div()
        .child(row)
        .when(open, move |this| match anchor {
            Some(b) => {
                let rows = items
                    .into_iter()
                    .map(|it| item_el(it, &flyout_theme))
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
