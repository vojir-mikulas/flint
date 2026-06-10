// SPDX-License-Identifier: GPL-3.0-or-later

//! `CodeEditor` — a multiline code-editing surface with syntax highlighting.
//!
//! Flint's `TextInput` is single-line and zed's `Editor` isn't in the pinned slim
//! `gpui`, so a multiline surface is genuinely net-new and is built the same
//! custom-`Element` way `TextInput` is: N shaped lines, a line-number gutter,
//! multi-line caret / selection / navigation, mouse hit-testing, and *live*
//! highlighting baked straight into each [`TextRun`]'s color.
//!
//! Domain-free by design — it takes a generic [`Highlighter`] callback (the SQL
//! tokenizer lives in RED) and an optional [`CompletionProvider`] seam (RED feeds
//! candidates from its introspected schema). The editor knows nothing about SQL.
//! Deferred: soft-wrap, undo, `tree-sitter`, horizontal scroll for long lines.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    actions, div, fill, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::theme::{ActiveTheme, Theme};

actions!(
    flint_code_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        Home,
        End,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectHome,
        SelectEnd,
        SelectAll,
        Newline,
        InsertTab,
        Run,
        Escape,
        Copy,
        Cut,
        Paste,
    ]
);

/// A token class the editor maps to a theme color. Generic (no SQL knowledge);
/// the caller's [`Highlighter`] produces these.
#[derive(Clone, Copy, PartialEq)]
pub enum TokenStyle {
    Keyword,
    String,
    Number,
    Function,
    Operator,
    Comment,
    Identifier,
}

impl TokenStyle {
    fn color(self, t: &Theme) -> Hsla {
        match self {
            TokenStyle::Keyword => t.purple,
            TokenStyle::String => t.green,
            TokenStyle::Number => t.orange,
            TokenStyle::Function => t.blue,
            TokenStyle::Operator => t.cyan,
            TokenStyle::Comment => t.text_faint,
            TokenStyle::Identifier => t.text,
        }
    }
}

/// Maps source text to `(byte range, style)` spans. Gaps render in the default
/// text color.
pub type Highlighter = Rc<dyn Fn(&str) -> Vec<(Range<usize>, TokenStyle)>>;

/// The completion seam: given the full text and the cursor's byte offset, return
/// candidate completions for the word ending at the cursor. The editor replaces
/// that word with the accepted candidate. Domain-free — RED supplies schema +
/// keyword candidates; the editor never knows what a "table" is.
pub type CompletionProvider = Rc<dyn Fn(&str, usize) -> Vec<SharedString>>;

/// Emitted so the owner reacts to ⌘↵ without the editor knowing what "run" means.
#[derive(Clone, Copy, Debug)]
pub enum CodeEditorEvent {
    Run,
}

/// An open completion popup: the word being completed starts at `start` (replaced
/// on accept), the matching `candidates`, and which one is highlighted.
struct Completion {
    start: usize,
    candidates: Vec<SharedString>,
    selected: usize,
}

pub struct CodeEditor {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    read_only: bool,
    /// Preserved column for vertical navigation, in bytes within a line.
    desired_col: Option<usize>,
    highlighter: Option<Highlighter>,
    completion_provider: Option<CompletionProvider>,
    completion: Option<Completion>,
    // Cached from the last paint, for hit-testing between paints.
    last_bounds: Option<Bounds<Pixels>>,
    last_lines: Vec<ShapedLine>,
    last_line_height: Pixels,
}

impl CodeEditor {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: String::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            is_selecting: false,
            read_only: false,
            desired_col: None,
            highlighter: None,
            completion_provider: None,
            completion: None,
            last_bounds: None,
            last_lines: Vec::new(),
            last_line_height: px(0.),
        }
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self.selected_range = 0..0;
        self
    }

    pub fn highlighter(
        mut self,
        f: impl Fn(&str) -> Vec<(Range<usize>, TokenStyle)> + 'static,
    ) -> Self {
        self.highlighter = Some(Rc::new(f));
        self
    }

    /// Install the completion seam. Candidates for the word under the cursor are
    /// recomputed as the user types; Tab / Enter accept the highlighted one.
    pub fn completions(mut self, f: impl Fn(&str, usize) -> Vec<SharedString> + 'static) -> Self {
        self.completion_provider = Some(Rc::new(f));
        self
    }

    /// Replace the completion provider after construction — RED rebuilds it as the
    /// schema loads, so candidates grow without recreating the editor.
    pub fn set_completions(
        &mut self,
        f: impl Fn(&str, usize) -> Vec<SharedString> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.completion_provider = Some(Rc::new(f));
        cx.notify();
    }

    pub fn content(&self) -> String {
        self.content.clone()
    }

    /// The current selection's text, or `None` if the selection is empty.
    pub fn selected_text(&self) -> Option<String> {
        (!self.selected_range.is_empty())
            .then(|| self.content[self.selected_range.clone()].to_string())
    }

    /// Replace the whole buffer (e.g. loading a query from history). Caret goes to
    /// the end; any open completion is dismissed.
    pub fn set_content(&mut self, content: impl Into<String>, cx: &mut Context<Self>) {
        self.content = content.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.completion = None;
        cx.notify();
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    /// Call once at startup. Bindings are scoped to the `"CodeEditor"` context.
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("CodeEditor");
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, ctx),
            KeyBinding::new("delete", Delete, ctx),
            KeyBinding::new("left", Left, ctx),
            KeyBinding::new("right", Right, ctx),
            KeyBinding::new("up", Up, ctx),
            KeyBinding::new("down", Down, ctx),
            KeyBinding::new("home", Home, ctx),
            KeyBinding::new("end", End, ctx),
            KeyBinding::new("shift-left", SelectLeft, ctx),
            KeyBinding::new("shift-right", SelectRight, ctx),
            KeyBinding::new("shift-up", SelectUp, ctx),
            KeyBinding::new("shift-down", SelectDown, ctx),
            KeyBinding::new("shift-home", SelectHome, ctx),
            KeyBinding::new("shift-end", SelectEnd, ctx),
            KeyBinding::new("enter", Newline, ctx),
            KeyBinding::new("tab", InsertTab, ctx),
            KeyBinding::new("escape", Escape, ctx),
        ]);
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, ctx),
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("cmd-x", Cut, ctx),
            KeyBinding::new("cmd-enter", Run, ctx),
        ]);
        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([
            KeyBinding::new("ctrl-a", SelectAll, ctx),
            KeyBinding::new("ctrl-c", Copy, ctx),
            KeyBinding::new("ctrl-v", Paste, ctx),
            KeyBinding::new("ctrl-x", Cut, ctx),
            KeyBinding::new("ctrl-enter", Run, ctx),
        ]);
    }

    // --- line / offset geometry ---

    /// Byte range of each line, excluding its trailing newline. Always ≥1 entry.
    fn line_ranges(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for (i, b) in self.content.bytes().enumerate() {
            if b == b'\n' {
                ranges.push(start..i);
                start = i + 1;
            }
        }
        ranges.push(start..self.content.len());
        ranges
    }

    fn line_col(&self, offset: usize) -> (usize, usize) {
        let ranges = self.line_ranges();
        for (i, r) in ranges.iter().enumerate() {
            if offset <= r.end {
                return (i, offset.saturating_sub(r.start));
            }
        }
        let last = ranges.len() - 1;
        (last, ranges[last].len())
    }

    // --- cursor / selection primitives (mirrors TextInput) ---

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.completion = None;
        cx.notify();
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

    fn vertical(&mut self, up: bool, extend: bool, cx: &mut Context<Self>) {
        let (line, col) = self.line_col(self.cursor_offset());
        let want = self.desired_col.unwrap_or(col);
        let ranges = self.line_ranges();
        let target = if up {
            line.checked_sub(1)
        } else if line + 1 < ranges.len() {
            Some(line + 1)
        } else {
            None
        };
        let offset = match target {
            Some(tl) => ranges[tl].start + want.min(ranges[tl].len()),
            None if up => 0,
            None => self.content.len(),
        };
        if extend {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        self.desired_col = Some(want);
    }

    // --- completion seam ---

    /// Byte offset where the word ending at the cursor begins, or `None` when no
    /// completion should be offered. Normally that's the start of the identifier
    /// under the cursor; as a special case, an empty word immediately after a `.`
    /// returns the cursor itself, so member-access completion (`table.` → that
    /// table's columns) fires before any suffix is typed. The replaced span is
    /// then empty and the candidate is inserted at the cursor.
    fn current_word_start(&self) -> Option<usize> {
        let cursor = self.cursor_offset();
        let bytes = self.content.as_bytes();
        let mut start = cursor;
        while start > 0 {
            let b = bytes[start - 1];
            if b.is_ascii_alphanumeric() || b == b'_' {
                start -= 1;
            } else {
                break;
            }
        }
        if start < cursor {
            return Some(start);
        }
        (start > 0 && bytes[start - 1] == b'.').then_some(cursor)
    }

    /// Recompute the completion popup against the provider. Called after edits.
    fn refresh_completions(&mut self) {
        self.completion = None;
        let Some(provider) = self.completion_provider.clone() else {
            return;
        };
        if !self.selected_range.is_empty() {
            return;
        }
        let Some(start) = self.current_word_start() else {
            return;
        };
        let candidates = provider(&self.content, self.cursor_offset());
        if !candidates.is_empty() {
            self.completion = Some(Completion {
                start,
                candidates,
                selected: 0,
            });
        }
    }

    fn completion_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(c) = &mut self.completion {
            let n = c.candidates.len() as isize;
            c.selected = (c.selected as isize + delta).rem_euclid(n) as usize;
            cx.notify();
        }
    }

    /// Replace the in-progress word with the highlighted candidate.
    fn accept_completion(&mut self, cx: &mut Context<Self>) -> bool {
        if self.read_only {
            return false;
        }
        let Some(c) = self.completion.take() else {
            return false;
        };
        let cursor = self.cursor_offset();
        let candidate = c.candidates[c.selected].to_string();
        self.content = self.content[..c.start].to_owned() + &candidate + &self.content[cursor..];
        let caret = c.start + candidate.len();
        self.selected_range = caret..caret;
        cx.notify();
        true
    }

    // --- action handlers ---

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        if self.selected_range.is_empty() {
            self.move_to(self.prev_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }
    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }
    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.select_to(self.prev_boundary(self.cursor_offset()), cx);
    }
    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }
    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if self.completion.is_some() {
            self.completion_move(-1, cx);
            return;
        }
        self.vertical(true, false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.completion.is_some() {
            self.completion_move(1, cx);
            return;
        }
        self.vertical(false, false, cx);
    }
    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(true, true, cx);
    }
    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(false, true, cx);
    }
    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        let (line, _) = self.line_col(self.cursor_offset());
        let start = self.line_ranges()[line].start;
        self.move_to(start, cx);
    }
    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        let (line, _) = self.line_col(self.cursor_offset());
        let end = self.line_ranges()[line].end;
        self.move_to(end, cx);
    }
    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.line_col(self.cursor_offset());
        let start = self.line_ranges()[line].start;
        self.select_to(start, cx);
    }
    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.line_col(self.cursor_offset());
        let end = self.line_ranges()[line].end;
        self.select_to(end, cx);
    }
    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let prev = self.prev_boundary(self.cursor_offset());
            if prev == self.cursor_offset() {
                return;
            }
            self.select_to(prev, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let next = self.next_boundary(self.cursor_offset());
            if next == self.cursor_offset() {
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        // Enter accepts an open completion rather than inserting a line.
        if self.accept_completion(cx) {
            return;
        }
        self.desired_col = None;
        self.replace_text_in_range(None, "\n", window, cx);
    }
    fn insert_tab(&mut self, _: &InsertTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.accept_completion(cx) {
            return;
        }
        self.desired_col = None;
        self.replace_text_in_range(None, "  ", window, cx);
    }
    fn run(&mut self, _: &Run, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(CodeEditorEvent::Run);
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.completion.take().is_some() {
            cx.notify();
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
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text, window, cx);
        }
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        self.desired_col = None;
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let offset = self.index_for_mouse_position(event.position);
            self.select_to(offset, cx);
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        let ranges = self.line_ranges();
        let lh = f32::from(self.last_line_height).max(1.0);
        let y_rel = f32::from(position.y - bounds.top()).max(0.0);
        let mut line = (y_rel / lh) as usize;
        if line >= ranges.len() {
            line = ranges.len() - 1;
        }
        let col = match self.last_lines.get(line) {
            Some(shaped) => shaped.closest_index_for_x(position.x - bounds.left()),
            None => 0,
        };
        (ranges[line].start + col).min(ranges[line].end)
    }

    fn prev_boundary(&self, offset: usize) -> usize {
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

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }
    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }
    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }
    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for CodeEditor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        let end = range.end.min(self.content.len());
        let start = range.start.min(end);
        let text = self.content.get(start..end)?;
        actual.replace(self.range_to_utf16(&(start..end)));
        Some(text.to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.range_to_utf16(r))
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
        if self.read_only {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.marked_range = None;
        self.refresh_completions();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.start)
            .unwrap_or_else(|| {
                let caret = range.start + new_text.len();
                caret..caret
            });
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (line, col) = self.line_col(range.start);
        let shaped = self.last_lines.get(line)?;
        let x = shaped.x_for_index(col);
        let y = bounds.top() + self.last_line_height * line as f32;
        Some(Bounds::from_corners(
            point(bounds.left() + x, y),
            point(bounds.left() + x, y + self.last_line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

/// The custom element: shapes every line with highlighted runs and paints the
/// text, caret and (multi-line) selection. Reads the owning [`CodeEditor`].
struct CodeElement {
    editor: Entity<CodeEditor>,
}

struct PrepaintState {
    lines: Vec<ShapedLine>,
    line_height: Pixels,
    cursor: Option<PaintQuad>,
    selections: Vec<PaintQuad>,
}

impl IntoElement for CodeElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for CodeElement {
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
        let line_count = self.editor.read(cx).content.split('\n').count().max(1);
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = (window.line_height() * line_count as f32).into();
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
        let theme = cx.theme().clone();
        let default_color = theme.text;
        let cursor_color = theme.accent;
        let selection_color = theme.bg_selected;

        let editor = self.editor.read(cx);
        let content = editor.content.clone();
        let selected = editor.selected_range.clone();
        let cursor = editor.cursor_offset();
        let tokens = editor
            .highlighter
            .as_ref()
            .map(|h| h(&content))
            .unwrap_or_default();
        let ranges = editor.line_ranges();

        let text_style = window.text_style();
        let font = text_style.font();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = window.line_height();

        // Shape each line with per-token colored runs (gaps fall back to default).
        let mut lines = Vec::with_capacity(ranges.len());
        for r in &ranges {
            let (ls, le) = (r.start, r.end);
            let mut runs: Vec<TextRun> = Vec::new();
            let mut pos = ls;
            for (tr, style) in &tokens {
                let s = tr.start.max(ls);
                let e = tr.end.min(le);
                if s >= e {
                    continue;
                }
                if s > pos {
                    runs.push(text_run(s - pos, &font, default_color));
                }
                runs.push(text_run(e - s, &font, style.color(&theme)));
                pos = e;
            }
            if pos < le {
                runs.push(text_run(le - pos, &font, default_color));
            }
            let line_text = SharedString::from(content[ls..le].to_string());
            lines.push(
                window
                    .text_system()
                    .shape_line(line_text, font_size, &runs, None),
            );
        }

        // Caret.
        let (cline, ccol) = line_col(&ranges, cursor);
        let caret_x = lines[cline].x_for_index(ccol);
        let caret = fill(
            Bounds::new(
                point(
                    bounds.left() + caret_x,
                    bounds.top() + line_height * cline as f32,
                ),
                size(px(2.), line_height),
            ),
            cursor_color,
        );

        // Selection: one quad per spanned line.
        let mut selections = Vec::new();
        if !selected.is_empty() {
            let (s_line, s_col) = line_col(&ranges, selected.start);
            let (e_line, e_col) = line_col(&ranges, selected.end);
            for (offset, shaped) in lines[s_line..=e_line].iter().enumerate() {
                let line = s_line + offset;
                let x0 = if line == s_line {
                    shaped.x_for_index(s_col)
                } else {
                    px(0.)
                };
                let x1 = if line == e_line {
                    shaped.x_for_index(e_col)
                } else {
                    // Extend slightly past EOL so full-line selections read as full.
                    shaped.width() + px(4.)
                };
                let top = bounds.top() + line_height * line as f32;
                selections.push(fill(
                    Bounds::from_corners(
                        point(bounds.left() + x0, top),
                        point(bounds.left() + x1, top + line_height),
                    ),
                    selection_color,
                ));
            }
        }

        PrepaintState {
            lines,
            line_height,
            cursor: Some(caret),
            selections,
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
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }

        let line_height = prepaint.line_height;
        let lines = std::mem::take(&mut prepaint.lines);
        for (i, line) in lines.iter().enumerate() {
            let origin = point(bounds.left(), bounds.top() + line_height * i as f32);
            let _ = line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx);
        }

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.editor.update(cx, |editor, _| {
            editor.last_bounds = Some(bounds);
            editor.last_lines = lines;
            editor.last_line_height = line_height;
        });
    }
}

fn text_run(len: usize, font: &gpui::Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// Free version of [`CodeEditor::line_col`] over precomputed ranges, for the
/// element (which holds ranges, not the entity).
fn line_col(ranges: &[Range<usize>], offset: usize) -> (usize, usize) {
    for (i, r) in ranges.iter().enumerate() {
        if offset <= r.end {
            return (i, offset.saturating_sub(r.start));
        }
    }
    let last = ranges.len() - 1;
    (last, ranges[last].len())
}

impl Focusable for CodeEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<CodeEditorEvent> for CodeEditor {}

impl Render for CodeEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let line_height = window.line_height();
        let line_count = self.content.split('\n').count().max(1);
        let gutter_fg = theme.text_faint;
        let border = theme.border_soft;

        // Line-number gutter (plain divs; scrolls with the code as a flex sibling).
        let gutter = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .border_r_1()
            .border_color(border)
            .children((1..=line_count).map(|n| {
                div()
                    .h(line_height)
                    .min_w(px(48.))
                    .px_3()
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_color(gutter_fg)
                    .child(n.to_string())
            }));

        let scroll_area =
            div()
                .id("code-scroll")
                .flex_1()
                .overflow_y_scroll()
                .py_2()
                .child(div().flex().items_start().child(gutter).child(
                    div().flex_1().pl_3().child(CodeElement {
                        editor: cx.entity(),
                    }),
                ));

        let focused = self.focus_handle.is_focused(window);

        // Completion popup (the seam, exercised). Docked bottom-left so it needs
        // no caret geometry; M-future may caret-anchor it.
        let popup = self.completion.as_ref().map(|c| {
            div()
                .absolute()
                .bottom_1()
                .left(px(56.))
                .max_w(px(280.))
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_sm)
                .py_1()
                .child(div().flex().flex_col().children(
                    c.candidates.iter().take(8).enumerate().map(|(i, cand)| {
                        div()
                            .px_2()
                            .py_0p5()
                            .text_size(px(12.))
                            .text_color(theme.text)
                            .when(i == c.selected, |d| d.bg(theme.bg_selected))
                            .child(cand.clone())
                    }),
                ))
        });

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_app)
            .border_1()
            .border_color(if focused { theme.accent } else { theme.border })
            .rounded(theme.radius)
            .overflow_hidden()
            .key_context("CodeEditor")
            .track_focus(&self.focus_handle(cx))
            .tab_index(0)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::insert_tab))
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .child(scroll_area)
            .children(popup)
    }
}
