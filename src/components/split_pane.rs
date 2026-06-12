// SPDX-License-Identifier: GPL-3.0-or-later

//! `SplitPane` - a two-pane resizable split. Stateless [`RenderOnce`]: the
//! consumer owns the sized pane's size and the in-flight drag anchor (GPUI
//! recreates elements each frame, so persistent state can't live here), exactly
//! as [`crate::Table`] leaves selection to the caller.
//!
//! One pane is sized in pixels; the other flexes to fill the rest. By default the
//! leading pane is the sized one; [`SplitPane::sized`] with [`SplitSide::Trailing`]
//! instead pins the trailing pane (for right/bottom-docked panels). Dragging the
//! divider reports a new pixel size for the sized pane via `on_resize`. While a
//! drag is in flight (`drag` is `Some`), the pane renders a full-cover overlay so
//! mouse moves are tracked even when the cursor leaves the thin divider.

use std::rc::Rc;

use gpui::{
    div, prelude::*, px, Along, AnyElement, App, Axis, MouseButton, MouseMoveEvent, Pixels,
    SharedString, Window,
};

use crate::theme::ActiveTheme;

/// Captured when a divider drag begins: where the cursor was (along the split
/// axis) and the leading pane's size at that moment. The overlay turns later
/// cursor positions into a new size as `start_size + (cursor - start_coord)`.
#[derive(Clone, Copy, Debug)]
pub struct DragAnchor {
    pub start_coord: Pixels,
    pub start_size: Pixels,
}

/// Which pane carries the explicit pixel `size` — the other one flexes to fill
/// the rest. `min_first`/`max_first` bound whichever pane is sized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SplitSide {
    /// The leading pane (left in `Horizontal`, top in `Vertical`) is sized.
    #[default]
    Leading,
    /// The trailing pane (right/bottom) is sized — for docked side panels.
    Trailing,
}

type ResizeHandler = Rc<dyn Fn(Pixels, &mut Window, &mut App)>;
type DragStartHandler = Rc<dyn Fn(DragAnchor, &mut Window, &mut App)>;
type DragEndHandler = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct SplitPane {
    id: SharedString,
    /// The axis the divider moves along: `Horizontal` = side-by-side panes,
    /// `Vertical` = stacked panes.
    axis: Axis,
    /// Which pane the `size`/`min_first`/`max_first` apply to.
    side: SplitSide,
    size: Pixels,
    min_first: Pixels,
    max_first: Option<Pixels>,
    drag: Option<DragAnchor>,
    first: Option<AnyElement>,
    second: Option<AnyElement>,
    on_resize: Option<ResizeHandler>,
    on_drag_start: Option<DragStartHandler>,
    on_drag_end: Option<DragEndHandler>,
}

impl SplitPane {
    pub fn new(id: impl Into<SharedString>, axis: Axis) -> Self {
        Self {
            id: id.into(),
            axis,
            side: SplitSide::Leading,
            size: px(280.),
            min_first: px(80.),
            max_first: None,
            drag: None,
            first: None,
            second: None,
            on_resize: None,
            on_drag_start: None,
            on_drag_end: None,
        }
    }

    /// Which pane carries the explicit pixel size (default [`SplitSide::Leading`]).
    pub fn sized(mut self, side: SplitSide) -> Self {
        self.side = side;
        self
    }

    /// Current size of the sized pane (caller-owned state).
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn min_first(mut self, min: Pixels) -> Self {
        self.min_first = min;
        self
    }

    pub fn max_first(mut self, max: Pixels) -> Self {
        self.max_first = Some(max);
        self
    }

    /// `Some(anchor)` while a drag is in flight — the caller stores the anchor
    /// handed to `on_drag_start` and clears it in `on_drag_end`.
    pub fn drag(mut self, drag: Option<DragAnchor>) -> Self {
        self.drag = drag;
        self
    }

    pub fn first(mut self, first: impl IntoElement) -> Self {
        self.first = Some(first.into_any_element());
        self
    }

    pub fn second(mut self, second: impl IntoElement) -> Self {
        self.second = Some(second.into_any_element());
        self
    }

    /// New clamped pixel size of the leading pane, fired during a drag.
    pub fn on_resize(mut self, handler: impl Fn(Pixels, &mut Window, &mut App) + 'static) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }

    /// Divider pressed — store the returned [`DragAnchor`] and pass it back via
    /// [`Self::drag`] until `on_drag_end`.
    pub fn on_drag_start(
        mut self,
        handler: impl Fn(DragAnchor, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag_start = Some(Rc::new(handler));
        self
    }

    /// Drag released (anywhere) — clear the stored anchor.
    pub fn on_drag_end(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_drag_end = Some(Rc::new(handler));
        self
    }

    fn clamp(&self, raw: Pixels) -> Pixels {
        let lo = f32::from(self.min_first);
        let hi = self.max_first.map(f32::from).unwrap_or(f32::MAX);
        px(f32::from(raw).clamp(lo, hi.max(lo)))
    }
}

impl RenderOnce for SplitPane {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let axis = self.axis;
        let horizontal = axis == Axis::Horizontal;
        let trailing = self.side == SplitSide::Trailing;
        let size = self.clamp(self.size);
        // A calm 1px separator centered in a wider grab gutter; the line picks up
        // the accent while the gutter is hovered (so the whole strip is grabbable
        // but the chrome stays a thin line, not a thick bar).
        let line_color = theme.border;
        let line_hover = theme.accent;

        // The sized pane gets the fixed pixel size; the other flexes. `trailing`
        // swaps which is which without moving the panes (first stays left/top).
        let sized = |children| {
            div()
                .flex_shrink_0()
                .overflow_hidden()
                .when(horizontal, |s| s.w(size).h_full())
                .when(!horizontal, |s| s.h(size).w_full())
                .children(children)
        };
        let flexed = |children| {
            div()
                .flex_1()
                .overflow_hidden()
                .min_w(px(0.))
                .min_h(px(0.))
                .children(children)
        };
        let (first, second) = if trailing {
            (flexed(self.first), sized(self.second))
        } else {
            (sized(self.first), flexed(self.second))
        };

        // Thin draggable gutter; press captures a drag anchor for the overlay.
        let cur_size = self.size;
        let on_drag_start = self.on_drag_start.clone();
        let line = div()
            .flex_shrink_0()
            .bg(line_color)
            .group_hover("flint-split", move |s| s.bg(line_hover))
            .when(horizontal, |s| s.w(px(1.)).h_full())
            .when(!horizontal, |s| s.h(px(1.)).w_full());
        let divider = div()
            .id(self.id.clone())
            .group("flint-split")
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .when(horizontal, |s| s.w(px(7.)).h_full().cursor_ew_resize())
            .when(!horizontal, |s| s.h(px(7.)).w_full().cursor_ns_resize())
            .child(line)
            .when_some(on_drag_start, |this, handler| {
                this.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                    handler(
                        DragAnchor {
                            start_coord: event.position.along(axis),
                            start_size: cur_size,
                        },
                        window,
                        cx,
                    )
                })
            });

        let container = div()
            .relative()
            .size_full()
            .flex()
            .when(horizontal, |s| s.flex_row())
            .when(!horizontal, |s| s.flex_col())
            .child(first)
            .child(divider)
            .child(second);

        // While dragging, a full-cover overlay tracks the cursor anywhere in the
        // split and ends the drag on release (inside or outside the overlay).
        container.when_some(self.drag, |this, anchor| {
            let overlay_id: SharedString = format!("{}-drag", self.id).into();
            let on_resize = self.on_resize.clone();
            let on_drag_end = self.on_drag_end.clone();
            let end = on_drag_end.clone();
            let min_first = self.min_first;
            let max_first = self.max_first;
            this.child(
                div()
                    .id(overlay_id)
                    .occlude()
                    .absolute()
                    .inset_0()
                    .when(horizontal, |s| s.cursor_ew_resize())
                    .when(!horizontal, |s| s.cursor_ns_resize())
                    .when_some(on_resize, |this, handler| {
                        this.on_mouse_move(move |event: &MouseMoveEvent, window, cx| {
                            let delta = event.position.along(axis) - anchor.start_coord;
                            // Dragging toward the trailing pane shrinks it, so the
                            // delta is subtracted when that's the sized one.
                            let raw = if trailing {
                                anchor.start_size - delta
                            } else {
                                anchor.start_size + delta
                            };
                            let lo = f32::from(min_first);
                            let hi = max_first.map(f32::from).unwrap_or(f32::MAX);
                            handler(px(f32::from(raw).clamp(lo, hi.max(lo))), window, cx);
                        })
                    })
                    .when_some(on_drag_end, |this, handler| {
                        this.on_mouse_up(MouseButton::Left, move |_, window, cx| {
                            handler(window, cx)
                        })
                    })
                    .when_some(end, |this, handler| {
                        this.on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                            handler(window, cx)
                        })
                    }),
            )
        })
    }
}
