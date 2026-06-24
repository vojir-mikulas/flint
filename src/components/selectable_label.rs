// SPDX-License-Identifier: GPL-3.0-or-later

//! `SelectableLabel` - read-only, wrapping text the user can highlight with the
//! mouse and copy (⌘/Ctrl+C). GPUI's plain text isn't selectable; this is the
//! lightweight, non-editable counterpart to [`TextInput`](super::text_input).
//!
//! It owns a focus handle — selection needs focus so the copy keybinding routes
//! here — and a byte selection range. An inner custom element wraps a
//! [`StyledText`], so all the line wrapping and hit-testing comes for free via
//! its [`TextLayout`]; this element only paints the selection highlight behind
//! it and republishes the layout for the next mouse event to hit-test against.
//!
//! Stateful: a [`Render`] view held in an `Entity`. Call
//! [`SelectableLabel::bind_keys`] once at startup. Text style (color, size,
//! font) is inherited from the parent, like any text element.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, prelude::*, App, Bounds, ClipboardItem, Context, CursorStyle,
    Element, ElementId, Entity, FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, SharedString, StyledText, TextLayout, Window,
};

use crate::theme::ActiveTheme;

actions!(flint_selectable_label, [Copy, SelectAll]);

/// A read-only, wrapping text label whose text can be selected and copied.
pub struct SelectableLabel {
    focus_handle: FocusHandle,
    text: SharedString,
    /// Byte range of the current selection (empty = nothing selected), always
    /// normalized to `start <= end`. The drag anchor is tracked separately so an
    /// upward/leftward drag still produces a forward range.
    selection: Range<usize>,
    /// The fixed end of an in-progress drag; the moving end follows the cursor.
    anchor: usize,
    selecting: bool,
    /// Captured each paint so the next mouse event can hit-test against the same
    /// geometry. Shares its `Rc` cell with the inner `StyledText`, so it stays
    /// live between frames.
    layout: Option<TextLayout>,
}

impl SelectableLabel {
    pub fn new(text: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            text: text.into(),
            selection: 0..0,
            anchor: 0,
            selecting: false,
            layout: None,
        }
    }

    pub fn text(&self) -> SharedString {
        self.text.clone()
    }

    /// Replace the text, clearing any selection.
    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.text = text.into();
        self.selection = 0..0;
        self.anchor = 0;
        self.layout = None;
        cx.notify();
    }

    /// Bind ⌘/Ctrl+C (copy selection) and ⌘/Ctrl+A (select all). Call once at
    /// startup, like [`TextInput::bind_keys`](super::text_input::TextInput::bind_keys).
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("SelectableLabel");
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-a", SelectAll, ctx),
        ]);
        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([
            KeyBinding::new("ctrl-c", Copy, ctx),
            KeyBinding::new("ctrl-a", SelectAll, ctx),
        ]);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.text.get(self.selection.clone()) {
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
            }
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.anchor = 0;
        self.selection = 0..self.text.len();
        cx.notify();
    }

    /// Byte index under `position` (or the nearest boundary); 0 before first paint.
    fn index_for(&self, position: Point<Pixels>) -> usize {
        self.layout
            .as_ref()
            .map(|layout| layout.index_for_position(position).unwrap_or_else(|i| i))
            .unwrap_or(0)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ix = self.index_for(event.position);
        self.anchor = ix;
        self.selection = ix..ix;
        self.selecting = true;
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selecting || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        let ix = self.index_for(event.position);
        self.selection = self.anchor.min(ix)..self.anchor.max(ix);
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }
}

impl Focusable for SelectableLabel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SelectableLabel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The focus handle doubles as a stable a11y id. Clicking the element
        // focuses the tracked handle (GPUI does this for focusable interactive
        // elements), so the `SelectableLabel`-context copy binding routes here.
        div()
            .id(ElementId::from(&self.focus_handle))
            .key_context("SelectableLabel")
            .track_focus(&self.focus_handle)
            .w_full()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::select_all))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .child(SelectionElement { view: cx.entity() })
    }
}

/// The inner custom element. It drives a [`StyledText`] for layout/paint (so
/// wrapping and hit-testing are GPUI's job), paints the selection highlight
/// behind the glyphs, and republishes the resulting [`TextLayout`] to the view.
struct SelectionElement {
    view: Entity<SelectableLabel>,
}

impl IntoElement for SelectionElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for SelectionElement {
    type RequestLayoutState = StyledText;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, StyledText) {
        // `StyledText` ignores the element/inspector ids, so passing `None` is
        // sound; it inherits the ambient text style for color/size/font.
        let mut text = StyledText::new(self.view.read(cx).text.clone());
        let (layout_id, ()) = text.request_layout(None, None, window, cx);
        (layout_id, text)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        text: &mut StyledText,
        window: &mut Window,
        cx: &mut App,
    ) {
        text.prepaint(None, None, bounds, &mut (), window, cx);
        // Republish the (now measured + bounded) layout so the entity's mouse
        // handlers can hit-test it on the next frame.
        let layout = text.layout().clone();
        self.view.update(cx, |view, _| view.layout = Some(layout));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        text: &mut StyledText,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let selection = self.view.read(cx).selection.clone();
        let layout = self.view.read(cx).layout.clone();
        if !selection.is_empty() {
            if let Some(layout) = layout {
                let color = cx.theme().bg_selected;
                for quad in selection_quads(&layout, selection, color) {
                    window.paint_quad(quad);
                }
            }
        }
        text.paint(None, None, bounds, &mut (), &mut (), window, cx);
    }
}

/// Highlight rectangles for `selection`, spanning soft- and hard-wrapped lines.
/// The first and middle visual rows extend to the right edge of the laid-out
/// text; the last row stops at the selection end — the conventional multi-line
/// selection look. Coordinates are absolute (the layout stores its paint bounds).
fn selection_quads(layout: &TextLayout, selection: Range<usize>, color: Hsla) -> Vec<PaintQuad> {
    let (Some(start), Some(end)) = (
        layout.position_for_index(selection.start),
        layout.position_for_index(selection.end),
    ) else {
        return Vec::new();
    };
    let line_height = layout.line_height();
    let bounds = layout.bounds();
    let (left, right) = (bounds.left(), bounds.right());
    let mut quads = Vec::new();

    // Same visual row: a single rectangle from start.x to end.x.
    if (end.y - start.y).abs() < line_height / 2. {
        quads.push(fill(
            Bounds::from_corners(start, point(end.x, start.y + line_height)),
            color,
        ));
        return quads;
    }

    // First (partial) row: from start.x to the right edge.
    quads.push(fill(
        Bounds::from_corners(start, point(right, start.y + line_height)),
        color,
    ));
    // Full middle rows.
    let mut y = start.y + line_height;
    while y < end.y - line_height / 2. {
        quads.push(fill(
            Bounds::from_corners(point(left, y), point(right, y + line_height)),
            color,
        ));
        y += line_height;
    }
    // Last (partial) row: from the left edge to end.x.
    quads.push(fill(
        Bounds::from_corners(point(left, end.y), point(end.x, end.y + line_height)),
        color,
    ));
    quads
}
