// SPDX-License-Identifier: GPL-3.0-or-later

//! `ScrimDismiss` - "click the dimmed backdrop to dismiss", minus the click that
//! ends a window drag.
//!
//! A modal/overlay scrim covers the whole window, including the draggable title
//! bar. On macOS a native title-bar drag still delivers a `mouse-up` to the view
//! — so a plain `on_click` dismisses at the end of *every* window move. The
//! cursor never moves *relative to* the window (the window follows it), so cursor
//! deltas can't tell a click from a drag; the only reliable signal is whether the
//! window itself moved between press and release.
//!
//! We stash the window origin on `mouse-down` and compare it on the click. The
//! stash is a thread-local (GPUI is single-threaded; an `occlude`d scrim is the
//! only element receiving these events) because a window move triggers a
//! re-render mid-drag, which would wipe any per-render state.

use std::cell::Cell;

use gpui::{App, MouseButton, Pixels, Point, StatefulInteractiveElement, Window};

thread_local! {
    static SCRIM_DOWN_ORIGIN: Cell<Option<Point<Pixels>>> = const { Cell::new(None) };
}

pub trait ScrimDismiss: StatefulInteractiveElement + Sized {
    /// Dismiss when the scrim is *clicked*, but not when the click merely ends a
    /// window drag begun on the scrim. Apply to the full-window scrim element
    /// (which must carry an `id` and usually `occlude`).
    fn on_scrim_dismiss(self, on_dismiss: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_mouse_down(MouseButton::Left, |_, window, _| {
            SCRIM_DOWN_ORIGIN.with(|cell| cell.set(Some(window.bounds().origin)));
        })
        .on_click(move |_, window, cx| {
            let down = SCRIM_DOWN_ORIGIN.with(|cell| cell.take());
            // Closed only when the window didn't move between press and release.
            // A missing down-origin (e.g. a synthetic click) counts as "no move".
            let dragged = down.is_some_and(|origin| origin != window.bounds().origin);
            if !dragged {
                on_dismiss(window, cx);
            }
        })
    }
}

impl<T: StatefulInteractiveElement> ScrimDismiss for T {}
