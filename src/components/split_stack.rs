//! `SplitStack` — an n-ary resizable split, the sibling of [`SplitPane`].
//!
//! [`SplitPane`] holds exactly two panes and sizes one of them in pixels, which
//! is right for a panel docked beside a flexible body: the dock keeps the width
//! the user gave it while the body absorbs the window. It is the wrong model for
//! a *row of equals* — with four columns, pixel sizing pins three of them and
//! lets the fourth absorb every resize.
//!
//! Here each child carries a fraction of its container, and a divider drag moves
//! only the two children it sits between. Widening the window widens every child
//! in proportion, and dragging one divider leaves the rest of the row untouched.
//!
//! Stateless apart from the in-flight drag, exactly as [`SplitPane`] is: GPUI
//! rebuilds elements each frame, so the weights and the drag anchor belong to the
//! caller.
//!
//! ```ignore
//! SplitStack::new("panes", Axis::Horizontal)
//!     .min(px(240.))
//!     .drag(self.divider)                     // caller-owned
//!     .child(0.5, left)
//!     .child(0.25, middle)
//!     .child(0.25, right)
//!     .on_drag_start(|drag, _, cx| { /* store it */ })
//!     .on_resize(|gutter, leading, _, cx| { /* set weights[gutter] */ })
//!     .on_drag_end(|_, cx| { /* clear it */ })
//! ```
//!
//! [`SplitPane`]: crate::SplitPane

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, Along, AnyElement, App, Axis, MouseButton, MouseMoveEvent, Pixels,
    SharedString, Window,
};

use crate::theme::ActiveTheme;

/// A divider drag in flight: which gap is being dragged, where the cursor
/// started, and the weight of the child before it at that moment.
///
/// The anchor is absolute rather than incremental — `on_resize` reports a *new*
/// weight rather than a delta — so a drag that outruns the frame rate, or a
/// clamp against a minimum, cannot accumulate drift.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DividerDrag {
    /// Index of the gutter: the gap after child `gutter`.
    pub gutter: usize,
    /// Cursor position along the split axis when the drag began.
    pub start: Pixels,
    /// Weight of the child before the gutter when the drag began.
    pub weight: f32,
}

type ResizeHandler = Rc<dyn Fn(usize, f32, &mut Window, &mut App)>;
type DragStartHandler = Rc<dyn Fn(DividerDrag, &mut Window, &mut App)>;
type DragEndHandler = Rc<dyn Fn(&mut Window, &mut App)>;

struct StackChild {
    weight: f32,
    element: AnyElement,
}

#[derive(IntoElement)]
pub struct SplitStack {
    id: SharedString,
    /// The axis the dividers move along: `Horizontal` = side-by-side children,
    /// `Vertical` = stacked ones.
    axis: Axis,
    /// Width of the draggable gutter the 1px separator sits in. Matches
    /// [`SplitPane`](crate::SplitPane)'s default so every divider in an app grabs
    /// the same way; `px(1.)` gives a flush, border-only divider.
    gutter: Pixels,
    /// Smallest a child may be squeezed to along the axis.
    min: Pixels,
    children: Vec<StackChild>,
    drag: Option<DividerDrag>,
    on_drag_start: Option<DragStartHandler>,
    on_resize: Option<ResizeHandler>,
    on_drag_end: Option<DragEndHandler>,
}

impl SplitStack {
    pub fn new(id: impl Into<SharedString>, axis: Axis) -> Self {
        Self {
            id: id.into(),
            axis,
            gutter: px(7.),
            min: px(120.),
            children: Vec::new(),
            drag: None,
            on_drag_start: None,
            on_resize: None,
            on_drag_end: None,
        }
    }

    /// Add a child occupying `weight` of the container. The caller's weights are
    /// expected to sum to 1; anything else still renders, just in the ratios
    /// given.
    pub fn child(mut self, weight: f32, element: impl IntoElement) -> Self {
        self.children.push(StackChild {
            weight,
            element: element.into_any_element(),
        });
        self
    }

    /// Smallest a child may be squeezed to along the axis (default `px(120.)`).
    pub fn min(mut self, min: Pixels) -> Self {
        self.min = min;
        self
    }

    /// Width of the draggable divider gutter (default `px(7.)`).
    pub fn gutter(mut self, gutter: Pixels) -> Self {
        self.gutter = gutter;
        self
    }

    /// `Some` while a divider of *this* stack is being dragged — the caller
    /// stores the anchor handed to `on_drag_start` and clears it in
    /// `on_drag_end`. Scoping it per stack is what keeps a nested stack's overlay
    /// from hijacking its parent's drag.
    pub fn drag(mut self, drag: Option<DividerDrag>) -> Self {
        self.drag = drag;
        self
    }

    /// A divider was pressed — store the returned [`DividerDrag`] and pass it
    /// back via [`Self::drag`] until `on_drag_end`.
    pub fn on_drag_start(
        mut self,
        handler: impl Fn(DividerDrag, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_drag_start = Some(Rc::new(handler));
        self
    }

    /// Fired during a drag with the gutter index and the new weight of the child
    /// *before* it. The caller clamps it and hands the remainder to the child
    /// after, so the pair's total is preserved and no other child moves.
    pub fn on_resize(
        mut self,
        handler: impl Fn(usize, f32, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_resize = Some(Rc::new(handler));
        self
    }

    /// Drag released (anywhere) — clear the stored anchor.
    pub fn on_drag_end(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_drag_end = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SplitStack {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (line_color, line_hover) = (theme.border, theme.accent);
        let axis = self.axis;
        let horizontal = axis == Axis::Horizontal;
        let (gutter, min) = (self.gutter, self.min);

        // The container's own extent along the axis, measured during paint and
        // read by the drag handler on the *same* frame's elements — GPUI paints,
        // then dispatches input against what it painted, so the cell is current
        // by the time a mouse move arrives. A frame-local `Rc<Cell<_>>` rather
        // than caller state: this is derived from layout, changes on every window
        // resize, and must never trigger a re-render of its own.
        let extent: Rc<Cell<f32>> = Rc::new(Cell::new(0.));
        let measure = extent.clone();
        let probe = canvas(
            move |bounds, _, _| {
                measure.set(f32::from(bounds.size.along(axis)));
            },
            |_, _, _, _| (),
        )
        .absolute()
        .size_full();

        let last = self.children.len().saturating_sub(1);
        let mut container = div()
            .id(self.id.clone())
            .relative()
            .size_full()
            .flex()
            .when(horizontal, |s| s.flex_row())
            .when(!horizontal, |s| s.flex_col())
            .child(probe);

        for (i, child) in self.children.into_iter().enumerate() {
            // `flex_basis(0)` with a per-child grow factor makes the grow factors
            // *be* the fractions: every pixel of the container is free space, so
            // it is handed out in exactly the caller's proportions.
            container = container.child(
                div()
                    .flex_grow(child.weight.max(0.001))
                    .flex_basis(px(0.))
                    .overflow_hidden()
                    .when(horizontal, |s| s.min_w(min).h_full())
                    .when(!horizontal, |s| s.min_h(min).w_full())
                    .child(child.element),
            );
            if i == last {
                break;
            }
            let line = div()
                .flex_shrink_0()
                .bg(line_color)
                .group_hover("flint-split-stack", move |s| s.bg(line_hover))
                .when(horizontal, |s| s.w(px(1.)).h_full())
                .when(!horizontal, |s| s.h(px(1.)).w_full());
            let start = self.on_drag_start.clone();
            let weight = child.weight;
            container = container.child(
                div()
                    .id((self.id.clone(), i))
                    .group("flint-split-stack")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(horizontal, |s| s.w(gutter).h_full().cursor_ew_resize())
                    .when(!horizontal, |s| s.h(gutter).w_full().cursor_ns_resize())
                    .child(line)
                    .when_some(start, |this, handler| {
                        this.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                            handler(
                                DividerDrag {
                                    gutter: i,
                                    start: event.position.along(axis),
                                    weight,
                                },
                                window,
                                cx,
                            )
                        })
                    }),
            );
        }

        // While dragging, a full-cover overlay tracks the cursor anywhere in the
        // stack and ends the drag on release, inside the overlay or out.
        container.when_some(self.drag, |this, anchor| {
            let overlay_id: SharedString = format!("{}-drag", self.id).into();
            let on_resize = self.on_resize.clone();
            let on_drag_end = self.on_drag_end.clone();
            let end_out = on_drag_end.clone();
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
                            let span = extent.get();
                            // Before the first paint of a drag there is nothing to
                            // divide by; skipping leaves the layout untouched.
                            if span <= 1. {
                                return;
                            }
                            let moved = f32::from(event.position.along(axis) - anchor.start) / span;
                            handler(anchor.gutter, anchor.weight + moved, window, cx);
                        })
                    })
                    .when_some(on_drag_end, |this, handler| {
                        this.on_mouse_up(MouseButton::Left, move |_, window, cx| {
                            handler(window, cx)
                        })
                    })
                    .when_some(end_out, |this, handler| {
                        this.on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                            handler(window, cx)
                        })
                    }),
            )
        })
    }
}
