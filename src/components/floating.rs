// SPDX-License-Identifier: GPL-3.0-or-later

//! `floating` — the one place that anchors a floating surface (menu, popover,
//! dropdown list, completion popup) into the window.
//!
//! Floating UI is easy to mis-place: hand-rolled `.absolute()` offsets ignore
//! where the trigger actually is, and a bare `anchored()` can spill off-screen or
//! get clipped by a scroll container. This wraps GPUI's `deferred` + `anchored`
//! with sane defaults — escapes clipping, snaps inside the viewport — so call
//! sites describe *where* the surface goes and nothing else.
//!
//! Two placement modes, both fit inside the window:
//! - **window-anchored** — [`Floating::at`] pins a corner to an explicit window
//!   point (a caret, a right-click position).
//! - **relative** — no `at`; the surface flows from its parent's top-left and is
//!   nudged by [`Floating::offset`] (drop a dropdown below its trigger).

use gpui::{anchored, deferred, prelude::*, px, Anchor, App, Pixels, Point, Window};

/// Default gap kept between the surface and the window edge when it would
/// otherwise overflow.
const DEFAULT_MARGIN: Pixels = px(8.);

/// A deferred, window-fitted floating surface. Build with [`floating`].
#[derive(IntoElement)]
pub struct Floating {
    child: gpui::AnyElement,
    anchor: Anchor,
    position: Option<Point<Pixels>>,
    offset: Point<Pixels>,
    margin: Pixels,
}

/// Wrap `child` as a floating surface. Defaults: top-left corner, relative
/// placement, 8px window margin. Chain [`Floating::at`] / [`Floating::anchor`] /
/// [`Floating::offset`] to position it.
pub fn floating(child: impl IntoElement) -> Floating {
    Floating {
        child: child.into_any_element(),
        anchor: Anchor::TopLeft,
        position: None,
        offset: Point::default(),
        margin: DEFAULT_MARGIN,
    }
}

impl Floating {
    /// Pin [`anchor`](Self::anchor) corner to this window-coordinate point.
    pub fn at(mut self, position: Point<Pixels>) -> Self {
        self.position = Some(position);
        self
    }

    /// Which corner of the surface sits at the anchor point / parent origin.
    pub fn anchor(mut self, anchor: Anchor) -> Self {
        self.anchor = anchor;
        self
    }

    /// Nudge the final position. In relative mode this is how a dropdown clears
    /// its trigger (e.g. `point(px(0.), px(36.))` for a 32px-tall trigger).
    pub fn offset(mut self, offset: Point<Pixels>) -> Self {
        self.offset = offset;
        self
    }

    /// Override the window-edge margin used when the surface would overflow.
    pub fn margin(mut self, margin: Pixels) -> Self {
        self.margin = margin;
        self
    }
}

impl RenderOnce for Floating {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let mut anchored = anchored()
            .anchor(self.anchor)
            .offset(self.offset)
            .snap_to_window_with_margin(self.margin)
            .child(self.child);
        if let Some(position) = self.position {
            anchored = anchored.position(position);
        }
        deferred(anchored)
    }
}
