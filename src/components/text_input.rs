// SPDX-License-Identifier: GPL-3.0-or-later

//! `TextInput` - a single-line, themed text field built in-house on GPUI's
//! text/element primitives. Owns cursor movement, selection, clipboard and
//! IME/marked-text. Stateful: a [`Render`] view held in an `Entity`; mechanics
//! ported from GPUI's `input` example. Call [`TextInput::bind_keys`] once at
//! startup.

use std::ops::Range;

use gpui::{
    actions, div, fill, point, prelude::*, px, size, App, Bounds, ClipboardItem, ContentMask,
    Context, CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler,
    EventEmitter, FocusHandle, Focusable, GlobalElementId, InspectorElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::styled_ext::StyledExt;
use crate::theme::ActiveTheme;

actions!(
    flint_text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        FocusNext,
        FocusPrev,
        Submit,
        Cancel,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

/// Emitted so the owner can react via `cx.subscribe` - the field can't know
/// whether submit means *create folder* or *save profile*.
#[derive(Clone, Copy, Debug)]
pub enum TextInputEvent {
    /// Enter - confirm the field's form.
    Submit,
    /// Escape - dismiss the field's form.
    Cancel,
    /// The content changed through user editing (typing, paste, delete, IME).
    /// Programmatic [`TextInput::set_content`] deliberately does **not** emit this,
    /// so owners can mirror one field into another without an echo loop.
    Change,
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// Horizontal pixel offset of the visible window into the (possibly wider)
    /// shaped line. Kept ≥ 0; recomputed each frame to follow the cursor so long
    /// single-line values stay reachable. See [`TextElement::prepaint`].
    scroll_offset: Pixels,
    /// Horizontal alignment of the text within the field. Only [`TextAlign::Left`]
    /// and [`TextAlign::Center`] are honoured (a single line never needs Right).
    align: TextAlign,
    /// The alignment shift baked into the last paint (0 unless centered and the
    /// text fits). Recorded so mouse/IME hit-testing inverts it. See `prepaint`.
    align_offset: Pixels,
    is_selecting: bool,
    obscured: bool,
    tab_stop: bool,
    /// Borderless, transparent-background variant for embedding in a styled
    /// container (e.g. a command palette's input row). The parent owns the chrome.
    bare: bool,
}

/// The glyph each character is shown as in an [obscured](TextInput::obscured)
/// field, and its UTF-8 length (used to map content offsets to display offsets).
const OBSCURE_GLYPH: &str = "•";

impl TextInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            scroll_offset: px(0.),
            align: TextAlign::Left,
            align_offset: px(0.),
            is_selecting: false,
            obscured: false,
            // Tab stop by default so `window.focus_next/prev` walks form fields.
            tab_stop: true,
            bare: false,
        }
    }

    /// Whether the field participates in Tab order (default `true`). Disable for
    /// standalone fields like a search box that shouldn't be in the tab ring.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        self
    }

    /// Drop the field's own border, background, padding and fixed height so it
    /// blends into a parent-styled row (e.g. a palette's seamless search input).
    /// The text size is inherited from the ambient style rather than forced.
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }

    /// Horizontal alignment of the text. Defaults to [`TextAlign::Left`]; pass
    /// [`TextAlign::Center`] for a centered value (e.g. a numeric stepper). When
    /// the text outgrows the field it falls back to left-aligned scrolling so the
    /// caret stays visible while typing.
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Replace the placeholder after construction (e.g. an embedded field whose
    /// owner sets it from a builder it received later). Does not emit `Change`.
    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.placeholder = placeholder.into();
        cx.notify();
    }

    /// Mask glyphs as bullets (password entry); [`content`](Self::content) still
    /// returns the real value.
    pub fn obscured(mut self) -> Self {
        self.obscured = true;
        self
    }

    pub fn with_content(mut self, content: impl Into<SharedString>) -> Self {
        self.content = content.into();
        self.selected_range = self.content.len()..self.content.len();
        self
    }

    pub fn content(&self) -> SharedString {
        self.content.clone()
    }

    pub fn set_content(&mut self, content: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = content.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    /// Call once at startup. Bindings are scoped to the `"TextInput"` key
    /// context, so they never leak into the rest of the app.
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("TextInput");
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, ctx),
            KeyBinding::new("delete", Delete, ctx),
            KeyBinding::new("left", Left, ctx),
            KeyBinding::new("right", Right, ctx),
            KeyBinding::new("shift-left", SelectLeft, ctx),
            KeyBinding::new("shift-right", SelectRight, ctx),
            KeyBinding::new("home", Home, ctx),
            KeyBinding::new("end", End, ctx),
            KeyBinding::new("shift-home", SelectHome, ctx),
            KeyBinding::new("shift-end", SelectEnd, ctx),
            // Tab traversal between fields (GPUI has no default `tab` binding).
            KeyBinding::new("tab", FocusNext, ctx),
            KeyBinding::new("shift-tab", FocusPrev, ctx),
            // Surfaced to the owner as `TextInputEvent`.
            KeyBinding::new("enter", Submit, ctx),
            KeyBinding::new("escape", Cancel, ctx),
        ]);

        // Clipboard, word-nav and line-nav modifiers differ by platform: macOS
        // uses cmd for clipboard and alt for word jumps; Windows/Linux use ctrl
        // for both. `cmd` maps to the Windows/Super key off macOS, so the mac
        // bindings are dead there — hence the split.
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("alt-delete", DeleteWordRight, ctx),
            KeyBinding::new("alt-left", WordLeft, ctx),
            KeyBinding::new("alt-right", WordRight, ctx),
            KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
            KeyBinding::new("cmd-a", SelectAll, ctx),
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("cmd-x", Cut, ctx),
            KeyBinding::new("cmd-left", Home, ctx),
            KeyBinding::new("cmd-right", End, ctx),
            KeyBinding::new("cmd-shift-left", SelectHome, ctx),
            KeyBinding::new("cmd-shift-right", SelectEnd, ctx),
            KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, ctx),
        ]);

        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("ctrl-delete", DeleteWordRight, ctx),
            KeyBinding::new("ctrl-left", WordLeft, ctx),
            KeyBinding::new("ctrl-right", WordRight, ctx),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, ctx),
            KeyBinding::new("ctrl-a", SelectAll, ctx),
            KeyBinding::new("ctrl-c", Copy, ctx),
            KeyBinding::new("ctrl-v", Paste, ctx),
            KeyBinding::new("ctrl-x", Cut, ctx),
        ]);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let prev = self.previous_word_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete_word_right(
        &mut self,
        _: &DeleteWordRight,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let next = self.next_word_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn focus_next(&mut self, _: &FocusNext, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_next(cx);
    }

    fn focus_prev(&mut self, _: &FocusPrev, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_prev(cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submit);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Cancel);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.previous_boundary(self.cursor_offset());
            if self.cursor_offset() == prev {
                window.play_system_bell();
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if self.cursor_offset() == next {
                window.play_system_bell();
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn show_character_palette(
        &mut self,
        _: &ShowCharacterPalette,
        window: &mut Window,
        _: &mut Context<Self>,
    ) {
        window.show_character_palette();
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // Single-line: collapse any pasted newlines to spaces.
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        // An obscured field shapes bullets, not the real text, so a hit-test on
        // the layout would yield offsets that don't line up with `content` (and
        // could split a multi-byte grapheme). Just place the cursor at the end.
        if self.obscured {
            return self.content.len();
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        line.closest_index_for_x(
            position.x - bounds.left() - self.align_offset + self.scroll_offset,
        )
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// Start of the word before `offset`. Unicode word segments, so leading
    /// whitespace/punctuation is skipped.
    fn previous_word_boundary(&self, offset: usize) -> usize {
        self.content
            .unicode_word_indices()
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    /// The end of the nearest word *after* `offset` (macOS alt-right).
    fn next_word_boundary(&self, offset: usize) -> usize {
        self.content
            .unicode_word_indices()
            .map(|(idx, word)| idx + word.len())
            .find(|&end| end > offset)
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        // Clamp to the content: an out-of-range or mid-codepoint IME/a11y query
        // must return `None`, not panic on the slice. `get` yields `None` unless
        // both ends land on char boundaries within bounds.
        let end = range.end.min(self.content.len());
        let start = range.start.min(end);
        let text = self.content.get(start..end)?;
        actual_range.replace(self.range_to_utf16(&(start..end)));
        Some(text.to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.marked_range.take();
        cx.notify();
        cx.emit(TextInputEvent::Change);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        } else {
            self.marked_range = None;
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .map(|new_range| new_range.start + range.start..new_range.end + range.end)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.notify();
        cx.emit(TextInputEvent::Change);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(
                bounds.left() + self.align_offset + last_layout.x_for_index(range.start)
                    - self.scroll_offset,
                bounds.top(),
            ),
            point(
                bounds.left() + self.align_offset + last_layout.x_for_index(range.end)
                    - self.scroll_offset,
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let last_layout = self.last_layout.as_ref()?;
        let utf8_index = last_layout
            .index_for_x(point.x - line_point.x - self.align_offset + self.scroll_offset)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

/// The custom element that shapes and paints the single line of text, plus the
/// cursor and selection highlight. It reads the owning [`TextInput`] entity.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    /// The scroll offset baked into `cursor`/`selection` this frame; the line is
    /// painted shifted left by the same amount.
    scroll_offset: Pixels,
    /// The alignment shift baked into this frame's paint positions.
    align_offset: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.theme();
        let placeholder_color = theme.text_dim;
        let cursor_color = theme.accent;
        let selection_color = theme.bg_selected;

        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let obscured = input.obscured;
        let marked_range = input.marked_range.clone();
        let prev_scroll = input.scroll_offset;
        let align = input.align;
        let style = window.text_style();

        let is_placeholder = content.is_empty();
        let (display_text, text_color) = if is_placeholder {
            (input.placeholder.clone(), placeholder_color)
        } else if obscured {
            // Mask each grapheme with a bullet; offsets are remapped below so the
            // cursor and selection still land in the right place.
            let count = content.graphemes(true).count();
            (SharedString::from(OBSCURE_GLYPH.repeat(count)), style.color)
        } else {
            (content.clone(), style.color)
        };

        // Map a content byte offset to the matching offset in `display_text`.
        let map_offset = |offset: usize| -> usize {
            if is_placeholder {
                0
            } else if obscured {
                let graphemes_before = content
                    .grapheme_indices(true)
                    .take_while(|(idx, _)| *idx < offset)
                    .count();
                graphemes_before * OBSCURE_GLYPH.len()
            } else {
                offset
            }
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        // IME marked-text styling only applies to a plain (non-obscured) field.
        let runs = match marked_range.as_ref() {
            Some(marked_range) if !obscured && !is_placeholder => vec![
                TextRun {
                    len: marked_range.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            _ => vec![run],
        };

        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(display_text, font_size, &runs, None);

        let cursor_pos = line.x_for_index(map_offset(cursor));

        // Follow the cursor: shift the visible window so the caret stays inside
        // `bounds`, then clamp so we never scroll past the text or before it. A
        // small caret margin keeps the 2px cursor from being clipped at the edges.
        let visible_width = bounds.size.width;
        let caret_margin = px(2.);
        let mut scroll_offset = prev_scroll;
        if cursor_pos < scroll_offset + caret_margin {
            scroll_offset = (cursor_pos - caret_margin).max(px(0.));
        } else if cursor_pos > scroll_offset + visible_width - caret_margin {
            scroll_offset = cursor_pos - visible_width + caret_margin;
        }
        let max_scroll = (line.width() + caret_margin - visible_width).max(px(0.));
        scroll_offset = scroll_offset.min(max_scroll).max(px(0.));

        // Center the line when it fits; once it outgrows the field the offset
        // collapses to 0 and we fall back to left-aligned scrolling. `scroll_offset`
        // is already clamped to 0 whenever the line fits, so the two never fight.
        let align_offset = match align {
            TextAlign::Center => ((visible_width - line.width()) * 0.5).max(px(0.)),
            _ => px(0.),
        };

        let cursor_pos = cursor_pos - scroll_offset + align_offset;
        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    cursor_color,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left()
                                + align_offset
                                + line.x_for_index(map_offset(selected_range.start))
                                - scroll_offset,
                            bounds.top(),
                        ),
                        point(
                            bounds.left()
                                + align_offset
                                + line.x_for_index(map_offset(selected_range.end))
                                - scroll_offset,
                            bounds.bottom(),
                        ),
                    ),
                    selection_color,
                )),
                None,
            )
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            scroll_offset,
            align_offset,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let scroll_offset = prepaint.scroll_offset;
        let align_offset = prepaint.align_offset;
        let line = prepaint.line.take().unwrap();
        // Clip to the field so a line wider than `bounds` doesn't spill into the
        // padding or past the rounded border; the line itself is shifted left by
        // the scroll offset computed in prepaint.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection);
            }
            line.paint(
                point(bounds.left() - scroll_offset + align_offset, bounds.top()),
                window.line_height(),
                gpui::TextAlign::Left,
                None,
                window,
                cx,
            )
            .unwrap();

            if focus_handle.is_focused(window) {
                if let Some(cursor) = prepaint.cursor.take() {
                    window.paint_quad(cursor);
                }
            }
        });

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll_offset = scroll_offset;
            input.align_offset = align_offset;
        });
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<TextInputEvent> for TextInput {}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        let mut field = div().flex().items_center();
        field = if self.bare {
            // Seamless: no box of our own; the parent row provides height,
            // padding and background. Text size inherits from the ambient style.
            field.w_full().text_color(cx.theme().text)
        } else {
            field
                .h(px(32.))
                .px(px(10.))
                .bg(cx.theme().bg_input)
                .border_1()
                .border_color(cx.theme().border)
                .rounded(cx.theme().radius)
                .text_size(cx.theme().font_size)
                .text_color(cx.theme().text)
        };
        let mut field = field
            .key_context("TextInput")
            .track_focus(&self.focus_handle(cx))
            .when(self.tab_stop, |this| this.tab_index(0))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::focus_next))
            .on_action(cx.listener(Self::focus_prev))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::show_character_palette))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(div().w_full().child(TextElement { input: cx.entity() }));
        // A bare field has no box to ring; its container owns any focus affordance.
        if focused && !self.bare {
            field = field.focus_ring(cx);
        }
        field
    }
}
