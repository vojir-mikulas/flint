// SPDX-License-Identifier: GPL-3.0-or-later

//! A single-select dropdown. Stateless: the caller owns both the selected index
//! and the open flag, reacting via [`on_toggle`](Select::on_toggle) and
//! [`on_select`](Select::on_select). The list is deferred+anchored so it floats
//! above clipping containers. The disclosure and check glyphs are caller-supplied
//! ([`chevron`](Select::chevron) / [`check`](Select::check)) so the component
//! stays domain-free, falling back to unicode marks when unset.

use std::rc::Rc;

use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, App, Bounds, FontWeight, Pixels, SharedString,
    Window,
};

use crate::components::floating::floating;
use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

type ToggleHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;
type SelectHandler = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Select {
    id: SharedString,
    options: Vec<SharedString>,
    selected: usize,
    open: bool,
    placeholder: SharedString,
    chevron: Option<AnyElement>,
    check: Option<AnyElement>,
    on_toggle: Option<ToggleHandler>,
    on_select: Option<SelectHandler>,
}

impl Select {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            options: Vec::new(),
            selected: 0,
            open: false,
            placeholder: "Select…".into(),
            chevron: None,
            check: None,
            on_toggle: None,
            on_select: None,
        }
    }

    pub fn option(mut self, label: impl Into<SharedString>) -> Self {
        self.options.push(label.into());
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected = index;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Shown when `selected` is out of range (no selection).
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Disclosure glyph for the trigger. Caller-supplied so the component stays
    /// domain-free; falls back to a stacked unicode chevron when unset.
    pub fn chevron(mut self, icon: impl IntoElement) -> Self {
        self.chevron = Some(icon.into_any_element());
        self
    }

    /// Mark glyph drawn on the selected row. Falls back to a unicode check.
    pub fn check(mut self, icon: impl IntoElement) -> Self {
        self.check = Some(icon.into_any_element());
        self
    }

    /// Trigger clicked, or the open list dismissed by an outside click.
    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }

    pub fn on_select(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_select = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Select {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme().clone();
        let open = self.open;
        let selected = self.selected;

        // The menu is anchored to the trigger's *measured* window bounds, not to a
        // layout-flow guess — a stateless `anchored()` in relative mode lands a
        // trigger-height too low. A `canvas` overlay records the trigger's bounds
        // each frame; the menu then drops from its bottom-left. (Mirrors how Zed's
        // `PopoverMenu` positions against `child_bounds`.)
        let bounds_state = window.use_keyed_state(
            SharedString::from(format!("{}__sel_bounds", self.id)),
            cx,
            |_, _| None::<Bounds<Pixels>>,
        );
        let trigger_bounds = *bounds_state.read(cx);
        let measure = bounds_state.clone();
        let on_toggle = self.on_toggle.map(Rc::new);
        let on_select = self.on_select.map(Rc::new);
        let ring = theme.accent;
        let glow = theme.accent_ghost;

        let has_selection = self.options.get(selected).is_some();
        let current = self
            .options
            .get(selected)
            .cloned()
            .unwrap_or_else(|| self.placeholder.clone());

        // Native macOS popup-button styling: the trigger sizes to its content
        // (not full width), reads the selection in the accent color, and carries
        // a stacked up/down chevron as its disclosure glyph.
        //
        // While open, the trigger carries no click handler: dismissal is owned by
        // the list's `on_mouse_down_out`, which would otherwise immediately reopen.
        // The disclosure glyph: the caller's lucide icon when supplied, else a
        // stacked unicode chevron so the domain-free gallery still renders.
        let disclosure = match self.chevron {
            Some(icon) => div().flex().items_center().child(icon).into_any_element(),
            None => div()
                .flex()
                .flex_col()
                .items_center()
                .text_color(theme.accent)
                .text_size(px(9.))
                .line_height(px(6.))
                .child("⌃")
                .child("⌄")
                .into_any_element(),
        };

        let trigger = div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(24.))
            .px_2()
            .rounded(theme.radius)
            .bg(theme.bg_input)
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(if has_selection {
                theme.accent
            } else {
                theme.text_faint
            })
            .cursor_pointer()
            .tab_index(0)
            .focus(move |s| s.focus_ring_color(ring, glow))
            .child(div().child(current))
            .child(disclosure)
            .child(
                // Invisible overlay that records the trigger's window bounds so the
                // menu can anchor to its bottom-left. Re-renders only on a change.
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
            )
            .when(!open, |this| {
                this.hover(|s| s.border_color(theme.border_strong))
                    .when_some(on_toggle.clone(), |this, toggle| {
                        this.on_click(move |_, window, cx| toggle(window, cx))
                    })
            });

        // One row per option, mirroring the ContextMenu item style. The selected
        // row reads in the accent color and shows a check. The caller's lucide
        // check (if any) is moved onto whichever row is selected; the rest fall
        // back to a unicode mark.
        let mut check = self.check;
        let rows = self.options.into_iter().enumerate().map(move |(ix, label)| {
            let is_selected = ix == selected;
            let handler = on_select.clone();
            let mark = is_selected.then(|| {
                check
                    .take()
                    .unwrap_or_else(|| div().text_xs().child("✓").into_any_element())
            });
            div()
                .id(ix)
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(4.))
                .text_xs()
                .text_color(if is_selected {
                    theme.accent
                } else {
                    theme.text
                })
                .cursor_pointer()
                .tab_index(0)
                .focus(move |s| s.bg(theme.bg_hover).focus_ring_color(ring, glow))
                .hover(move |s| s.bg(theme.bg_hover))
                .child(div().flex_1().child(label))
                .when_some(mark, |this, mark| this.child(mark))
                .when_some(handler, |this, handler| {
                    this.on_click(move |_, window, cx| handler(ix, window, cx))
                })
        });

        let list = div()
            .id("select-menu")
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(160.))
            // Cap the height and scroll so a long option list (e.g. every installed
            // font family) stays inside the viewport instead of running off-screen.
            .max_h(px(320.))
            .overflow_y_scroll()
            .p_1()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(px(7.))
            .shadow_lg()
            .when_some(on_toggle, |this, toggle| {
                this.on_mouse_down_out(move |_, window, cx| toggle(window, cx))
            })
            .children(rows);

        // Anchor the menu to the trigger's measured bottom-left (window coords),
        // dropped 4px clear of it. Until the first frame measures the trigger,
        // `trigger_bounds` is None and the menu simply waits a frame.
        div()
            .child(trigger)
            .when(open, |this| match trigger_bounds {
                Some(b) => this.child(
                    floating(list)
                        .at(b.bottom_left())
                        .offset(point(px(0.), px(4.))),
                ),
                None => this,
            })
    }
}
