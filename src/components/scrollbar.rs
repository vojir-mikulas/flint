// SPDX-License-Identifier: GPL-3.0-or-later

//! `Scrollbar` - a thin, draggable vertical position control that maps
//! **fraction 0..1**, deliberately decoupled from content height. A virtualized
//! list can claim millions of rows (whose pixel canvas exceeds what `f32`
//! placement can resolve); this control stays smooth regardless, because it
//! only ever speaks fractions of its own painted track.
//!
//! Stateless like the rest of Flint: the caller owns a [`ScrollbarState`]
//! (the in-flight drag) and reacts to [`on_scrub`](Scrollbar::on_scrub) by
//! scrolling its list / fetching the window there — the component never
//! touches the scrolled content itself.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, px, App, Bounds, DispatchPhase, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, SharedString, Window,
};

use crate::theme::ActiveTheme;

/// The track's width, and the thumb's minimum height as a fraction of the
/// track (a 47M-row list would otherwise yield a sub-pixel thumb).
const TRACK_W: Pixels = px(10.);
const MIN_THUMB: f32 = 0.03;

type ScrubHandler = Rc<dyn Fn(f32, &mut Window, &mut App) + 'static>;

/// Caller-owned drag state: `Some(grab)` while the thumb is being dragged,
/// where `grab` is the pointer's offset from the thumb's top edge (so the
/// thumb doesn't jump under the cursor on grab). Cheap to clone; store it on
/// the view next to the scroll handle it mirrors.
#[derive(Clone, Default)]
pub struct ScrollbarState(Rc<Cell<Option<Pixels>>>);

impl ScrollbarState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a drag is in flight (e.g. to suppress hover effects elsewhere).
    pub fn dragging(&self) -> bool {
        self.0.get().is_some()
    }
}

/// See the module docs. Render it inside a `relative()` container over the
/// scrolled content; it pins itself to the right edge.
#[derive(IntoElement)]
pub struct Scrollbar {
    id: SharedString,
    /// Scroll position 0..=1 (0 = top, 1 = scrolled to the end).
    fraction: f32,
    /// Viewport height over content height; `>= 1` hides the control.
    thumb_fraction: f32,
    state: ScrollbarState,
    on_scrub: Option<ScrubHandler>,
}

impl Scrollbar {
    pub fn new(id: impl Into<SharedString>, state: &ScrollbarState) -> Self {
        Self {
            id: id.into(),
            fraction: 0.,
            thumb_fraction: 1.,
            state: state.clone(),
            on_scrub: None,
        }
    }

    /// Current position as a fraction of the scrollable range (0 = top).
    pub fn fraction(mut self, fraction: f32) -> Self {
        self.fraction = fraction.clamp(0., 1.);
        self
    }

    /// Thumb size: the visible fraction of the content (viewport / content).
    /// `>= 1` (everything visible) renders nothing.
    pub fn thumb(mut self, thumb_fraction: f32) -> Self {
        self.thumb_fraction = thumb_fraction;
        self
    }

    /// Read `fraction`/`thumb` off a `uniform_list` scroll handle's last
    /// layout. Positions are derived from the list's pixel offset, so they're
    /// approximate past `f32`'s resolution — exactly the regime this control
    /// is built to tolerate.
    pub fn track_list(mut self, handle: &gpui::UniformListScrollHandle) -> Self {
        let state = handle.0.borrow();
        if let Some(size) = state.last_item_size {
            let content = f32::from(size.contents.height);
            let view = f32::from(size.item.height);
            if content > view && view > 0. {
                self.thumb_fraction = view / content;
                self.fraction =
                    (-f32::from(state.base_handle.offset().y) / (content - view)).clamp(0., 1.);
                return self;
            }
        }
        self.thumb_fraction = 1.;
        self
    }

    /// Called with the new fraction while the thumb is dragged (throttling is
    /// the caller's concern) and once on a track click.
    pub fn on_scrub(mut self, handler: impl Fn(f32, &mut Window, &mut App) + 'static) -> Self {
        self.on_scrub = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Scrollbar {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        if self.thumb_fraction >= 1. || self.on_scrub.is_none() {
            return div().id(self.id);
        }
        let thumb_fraction = self.thumb_fraction.max(MIN_THUMB);
        let fraction = self.fraction;
        let state = self.state.clone();
        let on_scrub = self.on_scrub.clone();

        // The thumb is positioned with relative lengths, so layout needs no
        // knowledge of the track's pixel height...
        let thumb_top = fraction * (1. - thumb_fraction);
        let dragging = state.dragging();
        let thumb = div()
            .absolute()
            .top(gpui::relative(thumb_top))
            .left(px(2.))
            .right(px(2.))
            .h(gpui::relative(thumb_fraction))
            .rounded(px(3.))
            .bg(if dragging {
                theme.text_faint
            } else {
                theme.border
            })
            .hover(|s| s.bg(theme.text_faint));

        // ...while hit-testing happens against the painted bounds, via this
        // canvas overlay (the same pattern as `Table`'s wheel arbitration).
        // Listeners are window-global, so a drag keeps tracking when the
        // pointer leaves the track mid-gesture.
        let track = canvas(
            |_, _, _| (),
            move |bounds: Bounds<Pixels>, _, window, _| {
                let track_h = bounds.size.height;
                let thumb_h = track_h * thumb_fraction;
                let range_h = track_h - thumb_h;
                let thumb_y = bounds.origin.y + range_h * fraction;
                let scrub = move |grab: Pixels,
                                  y: Pixels,
                                  window: &mut Window,
                                  cx: &mut App,
                                  handler: &ScrubHandler| {
                    if range_h > px(0.) {
                        let f = ((y - bounds.origin.y - grab) / range_h).clamp(0., 1.);
                        handler(f, window, cx);
                    }
                };

                let down_state = state.clone();
                let down_scrub = on_scrub.clone();
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble
                        || event.button != MouseButton::Left
                        || !bounds.contains(&event.position)
                    {
                        return;
                    }
                    // Grab the thumb where it was hit; a track click recenters
                    // the thumb on the pointer and scrubs there immediately.
                    let on_thumb = (thumb_y..thumb_y + thumb_h).contains(&event.position.y);
                    let grab = if on_thumb {
                        event.position.y - thumb_y
                    } else {
                        thumb_h / 2.
                    };
                    down_state.0.set(Some(grab));
                    if let Some(handler) = down_scrub.as_ref() {
                        if !on_thumb {
                            scrub(grab, event.position.y, window, cx, handler);
                        }
                    }
                    cx.stop_propagation();
                });

                let move_state = state.clone();
                let move_scrub = on_scrub.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Bubble {
                        return;
                    }
                    let Some(grab) = move_state.0.get() else {
                        return;
                    };
                    if let Some(handler) = move_scrub.as_ref() {
                        scrub(grab, event.position.y, window, cx, handler);
                    }
                    cx.stop_propagation();
                });

                let up_state = state.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                    if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                        return;
                    }
                    if up_state.0.take().is_some() {
                        cx.stop_propagation();
                    }
                });
            },
        )
        .absolute()
        .size_full();

        div()
            .id(self.id)
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(TRACK_W)
            .child(track)
            .child(thumb)
    }
}
