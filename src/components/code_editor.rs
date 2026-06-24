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
//! Deferred: soft-wrap, `tree-sitter`, horizontal scroll for long lines.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    actions, div, fill, point, prelude::*, px, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, GlobalElementId, Hsla, InspectorElementId, KeyBinding, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, Role,
    ScrollHandle, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::components::floating::floating;
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
        SoftNewline,
        InsertTab,
        Run,
        Escape,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        SelectNextOccurrence,
        DeleteWordLeft,
        DeleteWordRight,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
    ]
);

/// How many undo steps the editor retains. Older steps drop off the bottom.
const UNDO_LIMIT: usize = 200;

/// A point-in-time editor state, captured before an edit so undo can restore it.
#[derive(Clone)]
struct EditSnapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

/// What an edit was, so consecutive same-kind edits coalesce into one undo step
/// (typing a word is one undo, not one-per-keystroke). `Other` never coalesces.
#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

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

/// An optional companion to the [`CompletionProvider`]: maps a candidate string to
/// a short dim label shown beside it in the popup (e.g. a slash command's
/// description, or a column's type). Still domain-free — the editor only renders
/// whatever string the owner returns. `None` (the default) shows bare candidates.
pub type CompletionDetail = Rc<dyn Fn(&str) -> Option<SharedString>>;

/// A generic, domain-free completion category. The owner (e.g. RED) maps its own
/// concepts onto these; the editor renders a small coloured badge per kind and
/// never learns what a "table" is. [`CompletionKind::Text`] draws no badge.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CompletionKind {
    Keyword,
    Function,
    /// A member — a column, field, or property.
    Field,
    /// A named container — a table, struct, or namespace.
    Object,
    /// A type name.
    Type,
    /// A literal value or enum member.
    Value,
    /// Plain text — no badge (the default).
    #[default]
    Text,
}

impl CompletionKind {
    /// A short badge glyph and its accent colour, or `None` for [`Self::Text`].
    fn badge(self, t: &Theme) -> Option<(&'static str, Hsla)> {
        Some(match self {
            CompletionKind::Keyword => ("K", t.purple),
            CompletionKind::Function => ("ƒ", t.blue),
            CompletionKind::Field => ("▪", t.cyan),
            CompletionKind::Object => ("▦", t.text_muted),
            CompletionKind::Type => ("T", t.orange),
            CompletionKind::Value => ("V", t.green),
            CompletionKind::Text => return None,
        })
    }
}

/// A rich completion candidate: the inserted `label`, a [`CompletionKind`] badge,
/// an optional dim `detail` beside it (a column type, a function signature), and
/// optional `documentation` rendered in a side panel for the highlighted item.
/// Build via [`CompletionItem::new`] plus the chained setters.
#[derive(Clone)]
pub struct CompletionItem {
    pub label: SharedString,
    pub kind: CompletionKind,
    pub detail: Option<SharedString>,
    pub documentation: Option<SharedString>,
}

impl CompletionItem {
    pub fn new(label: impl Into<SharedString>, kind: CompletionKind) -> Self {
        Self {
            label: label.into(),
            kind,
            detail: None,
            documentation: None,
        }
    }

    /// A short dim label shown to the right of the candidate (type, signature…).
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A one-line guide rendered in the popup's doc panel while highlighted.
    pub fn documentation(mut self, doc: impl Into<SharedString>) -> Self {
        self.documentation = Some(doc.into());
        self
    }
}

/// A richer completion seam than [`CompletionProvider`]: returns [`CompletionItem`]s
/// that carry a kind badge, detail, and documentation. When set it supersedes the
/// plain string provider.
pub type RichCompletionProvider = Rc<dyn Fn(&str, usize) -> Vec<CompletionItem>>;

/// Emitted so the owner reacts to editor-level keys without the editor knowing
/// what they mean.
#[derive(Clone, Copy, Debug)]
pub enum CodeEditorEvent {
    /// ⌘↵ — the owner runs the buffer / selection.
    Run,
    /// Enter in a [`submit_on_enter`](CodeEditor::submit_on_enter) editor with no
    /// completion popup open — the owner sends/confirms (e.g. a chat composer).
    /// A plain multiline editor never emits this.
    Submit,
    /// Esc with no completion popup open — the owner can move focus elsewhere
    /// (the completion-dismiss case is handled internally and emits nothing).
    Escape,
}

/// An open completion popup: the word being completed starts at `start` (replaced
/// on accept), the matching `candidates`, and which one is highlighted.
struct Completion {
    start: usize,
    candidates: Vec<CompletionItem>,
    selected: usize,
}

/// The unit a mouse drag extends by, set by the initiating click: a single click
/// drags by character, a double-click by word, a triple-click by line.
#[derive(Clone, Copy, PartialEq)]
enum DragUnit {
    Char,
    Word,
    Line,
}

pub struct CodeEditor {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    /// The granularity an in-progress drag extends by, fixed by the click that
    /// started it. Paired with [`drag_anchor`](Self::drag_anchor).
    drag_unit: DragUnit,
    /// The word/line range the initiating double/triple-click selected; a drag
    /// extends the selection to the union of this and the unit under the cursor.
    drag_anchor: Option<Range<usize>>,
    read_only: bool,
    /// Dim prompt text shown when the buffer is empty (e.g. "Ask a question…").
    /// Purely a hint — never part of [`content`](Self::content). Empty by default.
    placeholder: SharedString,
    /// Whether to draw the line-number gutter (default `true`). Off for a prose
    /// surface like a chat composer, where line numbers are noise.
    gutter: bool,
    /// Whether Enter sends rather than inserting a line (default `false`). On, a
    /// bare Enter (no completion popup) emits [`CodeEditorEvent::Submit`] and
    /// Shift+Enter inserts a newline — the chat-composer convention.
    submit_on_enter: bool,
    /// Soft-wrap long lines to the editor width instead of scrolling horizontally
    /// (default `false`). For a prose surface like a chat composer. Off keeps the
    /// code-editor behaviour (one shaped line per logical line, horizontal scroll).
    soft_wrap: bool,
    /// Preserved column for vertical navigation, in bytes within a line.
    desired_col: Option<usize>,
    /// Corner radius of the editor frame; `None` uses `theme.radius`. `Some(px(0.))`
    /// gives square corners (see [`Self::corner_radius`]).
    corner_radius: Option<Pixels>,
    /// Whether to draw the resting (unfocused) frame border. `false` keeps the 1px
    /// box but makes it transparent while unfocused, for editors embedded in an
    /// already-bordered container (see [`Self::resting_border`]). The focus border
    /// is always drawn.
    resting_border: bool,
    highlighter: Option<Highlighter>,
    completion_provider: Option<CompletionProvider>,
    /// A richer provider that supersedes [`completion_provider`](Self::completion_provider)
    /// when set — its items carry a kind badge, detail, and documentation.
    rich_provider: Option<RichCompletionProvider>,
    /// Optional dim label shown beside each candidate (see [`CompletionDetail`]).
    completion_detail: Option<CompletionDetail>,
    completion: Option<Completion>,
    /// Accessible name reported to assistive technology (default "Code editor").
    a11y_label: SharedString,
    /// Undo/redo stacks of pre-edit snapshots. `undo` pops the most recent prior
    /// state; `redo` replays states undone since the last edit (cleared on a fresh
    /// edit). `last_edit` drives coalescing — see [`EditKind`].
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    last_edit: EditKind,
    /// Caret offset just after the last recorded edit; an edit contiguous with it
    /// extends the current undo run instead of starting a new one.
    last_edit_caret: usize,
    /// Drives the scroll container so caret-follow autoscroll can read the viewport
    /// and nudge the offset when the caret lands outside it.
    scroll_handle: ScrollHandle,
    /// Set by an edit or caret move; the next paint scrolls the caret back into view
    /// (then clears it). Gating on this — rather than scrolling every frame — lets the
    /// user wheel-scroll away from the caret without it snapping back.
    scroll_to_cursor: bool,
    // Cached from the last paint, for hit-testing between paints.
    last_bounds: Option<Bounds<Pixels>>,
    last_lines: Vec<ShapedLine>,
    /// Cached wrapped lines from the last paint (soft-wrap mode only), for
    /// wrap-aware hit-testing. Empty when `soft_wrap` is off.
    last_wrapped: Vec<WrappedLine>,
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
            drag_unit: DragUnit::Char,
            drag_anchor: None,
            read_only: false,
            placeholder: SharedString::default(),
            gutter: true,
            submit_on_enter: false,
            soft_wrap: false,
            desired_col: None,
            corner_radius: None,
            resting_border: true,
            highlighter: None,
            completion_provider: None,
            rich_provider: None,
            completion_detail: None,
            completion: None,
            a11y_label: SharedString::from("Code editor"),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: EditKind::Other,
            last_edit_caret: 0,
            scroll_handle: ScrollHandle::new(),
            scroll_to_cursor: false,
            last_bounds: None,
            last_lines: Vec::new(),
            last_wrapped: Vec::new(),
            last_line_height: px(0.),
        }
    }

    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self.selected_range = 0..0;
        self
    }

    /// Override the editor frame's corner radius (default: `theme.radius`). Pass
    /// `px(0.)` for square corners — e.g. an editor that fills a pane flush.
    pub fn corner_radius(mut self, radius: Pixels) -> Self {
        self.corner_radius = Some(radius);
        self
    }

    /// Whether to draw the resting (unfocused) frame border (default: `true`). Pass
    /// `false` when the editor already sits inside a bordered container, so its own
    /// border doesn't double up — the focus border is still drawn so focus stays
    /// visible, and the 1px box is preserved (just transparent) to avoid a layout
    /// shift on focus.
    pub fn resting_border(mut self, show: bool) -> Self {
        self.resting_border = show;
        self
    }

    /// The accessible name reported to assistive technology (default
    /// "Code editor"). Set it to the editor's role in context, e.g. "Query editor".
    pub fn a11y_label(mut self, label: impl Into<SharedString>) -> Self {
        self.a11y_label = label.into();
        self
    }

    /// Dim placeholder text shown while the buffer is empty (default: none).
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Whether to draw the line-number gutter (default `true`). Pass `false` for a
    /// prose surface — a chat composer, a comment box — where numbers are noise.
    pub fn gutter(mut self, show: bool) -> Self {
        self.gutter = show;
        self
    }

    /// Make Enter send rather than insert a line (default off). With it on, a bare
    /// Enter with no completion popup emits [`CodeEditorEvent::Submit`] and
    /// Shift+Enter inserts a newline — the chat-composer convention. Leave it off
    /// for a code surface, where Enter always inserts a line.
    pub fn submit_on_enter(mut self, submit: bool) -> Self {
        self.submit_on_enter = submit;
        self
    }

    /// Soft-wrap long lines to the editor width instead of scrolling horizontally
    /// (default off). For a prose surface — a chat composer, a comment box. Leave it
    /// off for code, where horizontal scroll preserves column alignment.
    pub fn soft_wrap(mut self, wrap: bool) -> Self {
        self.soft_wrap = wrap;
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

    /// Attach a dim detail label to each candidate in the popup (see
    /// [`CompletionDetail`]) — e.g. a slash command's description. Optional; without
    /// it the popup shows bare candidate strings.
    pub fn completion_detail(mut self, f: impl Fn(&str) -> Option<SharedString> + 'static) -> Self {
        self.completion_detail = Some(Rc::new(f));
        self
    }

    /// Install the rich completion seam ([`RichCompletionProvider`]). Items carry a
    /// [`CompletionKind`] badge, a dim `detail`, and `documentation` shown in the
    /// popup's doc panel. When set it supersedes [`Self::completions`].
    pub fn rich_completions(
        mut self,
        f: impl Fn(&str, usize) -> Vec<CompletionItem> + 'static,
    ) -> Self {
        self.rich_provider = Some(Rc::new(f));
        self
    }

    /// Replace the rich completion provider after construction — RED rebuilds it as
    /// the schema loads, so candidates grow without recreating the editor.
    pub fn set_rich_completions(
        &mut self,
        f: impl Fn(&str, usize) -> Vec<CompletionItem> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.rich_provider = Some(Rc::new(f));
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
        let content = content.into();
        // A wholesale replace (e.g. loading a query) is one undo step on its own.
        if content != self.content {
            self.record_undo(EditKind::Other);
        }
        self.content = content;
        let end = self.content.len();
        self.selected_range = end..end;
        self.last_edit_caret = end;
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
            // Shift+Enter always inserts a line — the soft-newline a
            // `submit_on_enter` composer needs, and harmless in a code surface.
            KeyBinding::new("shift-enter", SoftNewline, ctx),
            KeyBinding::new("tab", InsertTab, ctx),
            KeyBinding::new("escape", Escape, ctx),
        ]);
        // Word/line/document navigation and clipboard. macOS uses cmd for
        // clipboard + line/doc jumps and alt for word jumps; Windows/Linux use
        // ctrl for word jumps and ctrl-home/end for the document ends.
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, ctx),
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("cmd-x", Cut, ctx),
            KeyBinding::new("cmd-z", Undo, ctx),
            KeyBinding::new("cmd-shift-z", Redo, ctx),
            KeyBinding::new("cmd-enter", Run, ctx),
            // ⌘← / ⌘→ jump to the line's start / end (reusing Home / End).
            KeyBinding::new("cmd-left", Home, ctx),
            KeyBinding::new("cmd-right", End, ctx),
            KeyBinding::new("cmd-shift-left", SelectHome, ctx),
            KeyBinding::new("cmd-shift-right", SelectEnd, ctx),
            // ⌘↑ / ⌘↓ jump to the document's start / end.
            KeyBinding::new("cmd-up", DocStart, ctx),
            KeyBinding::new("cmd-down", DocEnd, ctx),
            KeyBinding::new("cmd-shift-up", SelectDocStart, ctx),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, ctx),
            // ⌥← / ⌥→ jump by word; ⌥⌫ / ⌥⌦ delete by word.
            KeyBinding::new("alt-left", WordLeft, ctx),
            KeyBinding::new("alt-right", WordRight, ctx),
            KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
            KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("alt-delete", DeleteWordRight, ctx),
            // ⌘D selects the word under the caret, then the next occurrence.
            KeyBinding::new("cmd-d", SelectNextOccurrence, ctx),
        ]);
        #[cfg(not(target_os = "macos"))]
        cx.bind_keys([
            KeyBinding::new("ctrl-a", SelectAll, ctx),
            KeyBinding::new("ctrl-c", Copy, ctx),
            KeyBinding::new("ctrl-v", Paste, ctx),
            KeyBinding::new("ctrl-x", Cut, ctx),
            KeyBinding::new("ctrl-z", Undo, ctx),
            KeyBinding::new("ctrl-y", Redo, ctx),
            KeyBinding::new("ctrl-shift-z", Redo, ctx),
            KeyBinding::new("ctrl-enter", Run, ctx),
            // Ctrl+← / Ctrl+→ jump by word; Ctrl+⌫ / Ctrl+Del delete by word.
            KeyBinding::new("ctrl-left", WordLeft, ctx),
            KeyBinding::new("ctrl-right", WordRight, ctx),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, ctx),
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("ctrl-delete", DeleteWordRight, ctx),
            // Ctrl+D selects the word under the caret, then the next occurrence.
            KeyBinding::new("ctrl-d", SelectNextOccurrence, ctx),
            // Ctrl+Home / Ctrl+End jump to the document's start / end.
            KeyBinding::new("ctrl-home", DocStart, ctx),
            KeyBinding::new("ctrl-end", DocEnd, ctx),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, ctx),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, ctx),
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

    /// The caret's byte offset into [`content`](Self::content) (the selection's
    /// active end). Lets a host scope an action to the caret — e.g. running just
    /// the statement under the cursor.
    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    /// Move the caret to `offset` (clamped to the content and snapped to a char
    /// boundary), collapsing any selection and scrolling it into view. Lets a host
    /// place the cursor after a programmatic [`set_content`](Self::set_content) —
    /// e.g. restoring position after a line move.
    pub fn set_cursor(&mut self, offset: usize, cx: &mut Context<Self>) {
        let mut o = offset.min(self.content.len());
        while o > 0 && !self.content.is_char_boundary(o) {
            o -= 1;
        }
        self.selected_range = o..o;
        self.selection_reversed = false;
        self.last_edit_caret = o;
        self.scroll_to_cursor = true;
        cx.notify();
    }

    /// Select the byte span `start..end` (each clamped to the content and snapped
    /// to a char boundary) and scroll it into view, leaving the caret at `end`.
    /// Lets a host highlight a programmatically-found span — e.g. the current
    /// match of a find-in-editor search — without the host tracking layout.
    pub fn select_range(&mut self, start: usize, end: usize, cx: &mut Context<Self>) {
        let snap = |mut o: usize| {
            o = o.min(self.content.len());
            while o > 0 && !self.content.is_char_boundary(o) {
                o -= 1;
            }
            o
        };
        let (s, e) = (snap(start), snap(end));
        self.selected_range = s.min(e)..s.max(e);
        self.selection_reversed = false;
        self.last_edit_caret = self.selected_range.end;
        self.scroll_to_cursor = true;
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.completion = None;
        self.scroll_to_cursor = true;
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
        self.scroll_to_cursor = true;
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
        // Empty-word triggers: a `.` (member access) or a `/` (slash command) right
        // before the cursor offers completion before any suffix is typed, so the
        // popup opens the instant the user types the trigger. The provider decides
        // whether there's anything to show, so non-completion uses (SQL division,
        // file paths) simply yield no popup.
        (start > 0 && matches!(bytes[start - 1], b'.' | b'/')).then_some(cursor)
    }

    /// Recompute the completion popup against the provider. Called after edits.
    /// The rich provider wins when present; otherwise the plain string provider's
    /// candidates are wrapped (kind [`CompletionKind::Text`], with any
    /// [`CompletionDetail`] applied) so both seams render through one path.
    fn refresh_completions(&mut self) {
        self.completion = None;
        if !self.selected_range.is_empty() {
            return;
        }
        let Some(start) = self.current_word_start() else {
            return;
        };
        let offset = self.cursor_offset();
        let candidates: Vec<CompletionItem> = if let Some(provider) = self.rich_provider.clone() {
            provider(&self.content, offset)
        } else if let Some(provider) = self.completion_provider.clone() {
            let detail = self.completion_detail.clone();
            provider(&self.content, offset)
                .into_iter()
                .map(|label| {
                    let d = detail.as_ref().and_then(|f| f(&label));
                    CompletionItem {
                        label,
                        kind: CompletionKind::Text,
                        detail: d,
                        documentation: None,
                    }
                })
                .collect()
        } else {
            return;
        };
        if !candidates.is_empty() {
            self.completion = Some(Completion {
                start,
                candidates,
                selected: 0,
            });
        }
    }

    /// Window-coordinate point just below the start of the word being completed,
    /// where the popup's top-left should sit. Derived from the geometry cached at
    /// the last paint (`last_bounds` is the text element's origin in window
    /// space), so it tracks the caret and scrolls with the text. `None` until the
    /// editor has painted, or when there's no open completion.
    fn completion_anchor(&self) -> Option<Point<Pixels>> {
        let c = self.completion.as_ref()?;
        let bounds = self.last_bounds?;
        if self.last_lines.is_empty() {
            return None;
        }
        let (line, col) = self.line_col(c.start);
        let line = line.min(self.last_lines.len() - 1);
        let x = self.last_lines[line].x_for_index(col);
        // Drop one line below the word's line, plus a small gap off the caret.
        let y = self.last_line_height * (line as f32 + 1.0) + px(2.);
        Some(bounds.origin + point(x, y))
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
        let candidate = c.candidates[c.selected].label.to_string();
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

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.move_to(self.prev_word_boundary(self.cursor_offset()), cx);
    }
    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }
    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.select_to(self.prev_word_boundary(self.cursor_offset()), cx);
    }
    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }
    /// ⌘D / Ctrl+D: with no selection, select the word under the caret; with one,
    /// jump it to the next occurrence of that text (wrapping around the buffer).
    fn select_next_occurrence(
        &mut self,
        _: &SelectNextOccurrence,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.desired_col = None;
        if self.selected_range.is_empty() {
            let word = self.word_at(self.cursor_offset());
            if !word.is_empty() {
                self.selected_range = word;
                self.selection_reversed = false;
                self.completion = None;
                self.scroll_to_cursor = true;
                cx.notify();
            }
            return;
        }
        let needle = self.content[self.selected_range.clone()].to_string();
        let from = self.selected_range.end;
        let found = self.content[from..]
            .find(&needle)
            .map(|i| from + i)
            .or_else(|| self.content.find(&needle));
        if let Some(start) = found {
            self.selected_range = start..start + needle.len();
            self.selection_reversed = false;
            self.completion = None;
            self.scroll_to_cursor = true;
            cx.notify();
        }
    }
    fn delete_word_left(
        &mut self,
        _: &DeleteWordLeft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let prev = self.prev_word_boundary(self.cursor_offset());
            if prev == self.cursor_offset() {
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
            if next == self.cursor_offset() {
                return;
            }
            self.select_to(next, cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }
    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.move_to(0, cx);
    }
    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.move_to(self.content.len(), cx);
    }
    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.select_to(0, cx);
    }
    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.select_to(self.content.len(), cx);
    }

    /// Start of the nearest word *before* `offset` (mac ⌥←, Linux Ctrl+←).
    fn prev_word_boundary(&self, offset: usize) -> usize {
        self.content
            .unicode_word_indices()
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }
    /// End of the nearest word *after* `offset` (mac ⌥→, Linux Ctrl+→).
    fn next_word_boundary(&self, offset: usize) -> usize {
        self.content
            .unicode_word_indices()
            .map(|(idx, word)| idx + word.len())
            .find(|&end| end > offset)
            .unwrap_or(self.content.len())
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
        // In a composer, a bare Enter sends instead of inserting a line; the
        // owner reacts to `Submit`. Shift+Enter (a separate action) still inserts.
        if self.submit_on_enter {
            cx.emit(CodeEditorEvent::Submit);
            return;
        }
        self.desired_col = None;
        self.replace_text_in_range(None, "\n", window, cx);
    }
    /// Shift+Enter: always insert a newline, regardless of `submit_on_enter`.
    fn soft_newline(&mut self, _: &SoftNewline, window: &mut Window, cx: &mut Context<Self>) {
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
        // Esc first dismisses an open completion popup; with none open it's the
        // owner's to act on (e.g. move focus out of the editor).
        if self.completion.take().is_some() {
            cx.notify();
        } else {
            cx.emit(CodeEditorEvent::Escape);
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

    /// Record the pre-edit state for undo, unless this edit coalesces with the
    /// current run (same kind, contiguous caret). Call *before* mutating `content`.
    /// Any edit invalidates the redo stack.
    fn record_undo(&mut self, kind: EditKind) {
        let caret = self.selected_range.start;
        let contiguous = match kind {
            // Typing extends a run when each char lands at the previous caret.
            EditKind::Insert => self.selected_range.is_empty() && caret == self.last_edit_caret,
            // Backspace (range.end == caret) and forward-delete both stay in-run.
            EditKind::Delete => {
                self.selected_range.end == self.last_edit_caret
                    || self.selected_range.start == self.last_edit_caret
            }
            EditKind::Other => false,
        };
        if !(contiguous && kind == self.last_edit) {
            self.undo_stack.push(EditSnapshot {
                content: self.content.clone(),
                selected_range: self.selected_range.clone(),
                selection_reversed: self.selection_reversed,
            });
            if self.undo_stack.len() > UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
        }
        self.last_edit = kind;
        self.redo_stack.clear();
    }

    /// Classify an edit for undo coalescing from its range and replacement text.
    fn edit_kind(range: &Range<usize>, new_text: &str) -> EditKind {
        if new_text.is_empty() {
            EditKind::Delete
        } else if range.is_empty()
            && !new_text.contains('\n')
            && new_text.graphemes(true).count() == 1
        {
            EditKind::Insert
        } else {
            // Paste, newline, tab, or replacing a selection — each its own step.
            EditKind::Other
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(EditSnapshot {
                content: self.content.clone(),
                selected_range: self.selected_range.clone(),
                selection_reversed: self.selection_reversed,
            });
            self.apply_snapshot(prev);
            cx.notify();
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(EditSnapshot {
                content: self.content.clone(),
                selected_range: self.selected_range.clone(),
                selection_reversed: self.selection_reversed,
            });
            self.apply_snapshot(next);
            cx.notify();
        }
    }

    /// Restore a snapshot and break the coalescing run so the next keystroke
    /// starts a fresh undo step. Drops any open completion and IME marking.
    fn apply_snapshot(&mut self, snap: EditSnapshot) {
        self.content = snap.content;
        self.selected_range = snap.selected_range;
        self.selection_reversed = snap.selection_reversed;
        self.marked_range = None;
        self.desired_col = None;
        self.completion = None;
        self.last_edit = EditKind::Other;
        self.last_edit_caret = self.selected_range.start;
        self.refresh_completions();
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        self.desired_col = None;
        let offset = self.index_for_mouse_position(event.position);
        match event.click_count {
            // Triple-click: select the line; dragging then extends line by line.
            n if n >= 3 => {
                let line = self.line_at(offset);
                self.drag_unit = DragUnit::Line;
                self.drag_anchor = Some(line.clone());
                self.selected_range = line;
                self.selection_reversed = false;
                self.completion = None;
                cx.notify();
            }
            // Double-click: select the word; dragging then extends word by word.
            2 => {
                let word = self.word_at(offset);
                self.drag_unit = DragUnit::Word;
                self.drag_anchor = Some(word.clone());
                self.selected_range = word;
                self.selection_reversed = false;
                self.completion = None;
                cx.notify();
            }
            // Single click: place the caret (Shift extends); a drag extends by char.
            _ => {
                self.drag_unit = DragUnit::Char;
                self.drag_anchor = None;
                if event.modifiers.shift {
                    self.select_to(offset, cx);
                } else {
                    self.move_to(offset, cx);
                }
            }
        }
    }
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        let offset = self.index_for_mouse_position(event.position);
        match (self.drag_unit, self.drag_anchor.clone()) {
            (DragUnit::Word, Some(anchor)) => {
                let unit = self.word_at(offset);
                self.extend_union(anchor, unit, cx);
            }
            (DragUnit::Line, Some(anchor)) => {
                let unit = self.line_at(offset);
                self.extend_union(anchor, unit, cx);
            }
            _ => self.select_to(offset, cx),
        }
    }

    /// The byte range of the word containing (or adjacent to) `offset`. Empty when
    /// the click lands in whitespace between words.
    fn word_at(&self, offset: usize) -> Range<usize> {
        let off = offset.min(self.content.len());
        for (idx, word) in self.content.unicode_word_indices() {
            let end = idx + word.len();
            if off >= idx && off <= end {
                return idx..end;
            }
            if off < idx {
                break;
            }
        }
        off..off
    }

    /// The byte range of the line containing `offset` (excluding its newline).
    fn line_at(&self, offset: usize) -> Range<usize> {
        let (line, _) = self.line_col(offset);
        self.line_ranges()[line].clone()
    }

    /// Set the selection to the union of the drag `anchor` and the `unit` under the
    /// cursor, reversed when dragging back before the anchor.
    fn extend_union(&mut self, anchor: Range<usize>, unit: Range<usize>, cx: &mut Context<Self>) {
        self.selected_range = anchor.start.min(unit.start)..anchor.end.max(unit.end);
        self.selection_reversed = unit.start < anchor.start;
        self.scroll_to_cursor = true;
        cx.notify();
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        let ranges = self.line_ranges();
        let line_height = self.last_line_height;
        let lh = f32::from(line_height).max(1.0);
        let y_rel = f32::from(position.y - bounds.top()).max(0.0);

        // Soft-wrap: a logical line can span several visual rows, so walk the cached
        // wrapped lines by their stacked heights and let GPUI map the local point to
        // a byte index within the line (it accounts for the wrap rows).
        if self.soft_wrap && !self.last_wrapped.is_empty() {
            let mut top = 0.0_f32;
            for (line, wl) in self.last_wrapped.iter().enumerate() {
                let h = f32::from(wl.size(line_height).height).max(lh);
                let range = ranges.get(line).cloned().unwrap_or(0..self.content.len());
                if y_rel < top + h || line + 1 == self.last_wrapped.len() {
                    let local = point(position.x - bounds.left(), px(y_rel - top));
                    let col = wl
                        .closest_index_for_position(local, line_height)
                        .unwrap_or_else(|e| e);
                    return (range.start + col).min(range.end);
                }
                top += h;
            }
            return self.content.len();
        }

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
        // Snapshot for undo before the buffer changes (coalescing consecutive
        // typing / deletion into one step).
        self.record_undo(Self::edit_kind(&range, new_text));
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.last_edit_caret = caret;
        self.marked_range = None;
        self.scroll_to_cursor = true;
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
        // IME composition snapshots once, when marking begins (not on every
        // intermediate update of the same marked region).
        if self.marked_range.is_none() {
            self.record_undo(EditKind::Other);
        }
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.last_edit_caret = range.start + new_text.len();
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
        self.scroll_to_cursor = true;
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
    /// Soft-wrap mode: one wrapped layout per logical line, paired with its top
    /// offset. Empty when `soft_wrap` is off (then `lines` is used).
    wrapped: Vec<WrappedLine>,
    tops: Vec<Pixels>,
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
        let editor = self.editor.read(cx);
        let line_height = window.line_height();
        let line_count = editor.content.split('\n').count().max(1);
        // Soft-wrap height depends on the available width. We don't have this frame's
        // width yet, so reuse the previous paint's width to count visual rows; on the
        // first frame (no cached width) fall back to the logical line count, which the
        // next frame corrects. The visual-row count is an upper bound on logical
        // lines, so the scroll container never under-sizes for long content.
        let rows = if editor.soft_wrap {
            match editor.last_bounds {
                Some(b) if b.size.width > px(0.) => {
                    visual_row_count(&editor.content, b.size.width, line_height, window)
                        .max(line_count)
                }
                _ => line_count,
            }
        } else {
            line_count
        };
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = (line_height * rows as f32).into();
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

        // --- soft-wrap path: one wrapped layout per logical line, stacked ---
        if editor.soft_wrap {
            // Runs over the whole content (token colors; gaps default), as shape_text
            // splits on newlines itself and wants runs covering the full text.
            let mut runs: Vec<TextRun> = Vec::new();
            let mut pos = 0usize;
            for (tr, style) in &tokens {
                let s = tr.start.min(content.len());
                let e = tr.end.min(content.len());
                if s >= e || s < pos {
                    continue;
                }
                if s > pos {
                    runs.push(text_run(s - pos, &font, default_color));
                }
                runs.push(text_run(e - s, &font, style.color(&theme)));
                pos = e;
            }
            if pos < content.len() {
                runs.push(text_run(content.len() - pos, &font, default_color));
            }
            if runs.is_empty() {
                runs.push(text_run(content.len(), &font, default_color));
            }

            let wrap_width = bounds.size.width;
            let wrapped: Vec<WrappedLine> = window
                .text_system()
                .shape_text(
                    SharedString::from(content.clone()),
                    font_size,
                    &runs,
                    Some(wrap_width),
                    None,
                )
                .unwrap_or_default()
                .into_iter()
                .collect();

            // Top offset of each logical line (its predecessors' wrapped heights).
            let mut tops: Vec<Pixels> = Vec::with_capacity(wrapped.len());
            let mut acc = px(0.);
            for wl in &wrapped {
                tops.push(acc);
                acc += wl.size(line_height).height;
            }

            // Caret, mapped through the wrapped layout (visual row + x within it).
            let (cline, ccol) = line_col(&ranges, cursor);
            let caret = wrapped.get(cline).map(|wl| {
                let local = wl
                    .position_for_index(ccol, line_height)
                    .unwrap_or(point(px(0.), px(0.)));
                fill(
                    Bounds::new(
                        point(
                            bounds.left() + local.x,
                            bounds.top() + tops[cline] + local.y,
                        ),
                        size(px(2.), line_height),
                    ),
                    cursor_color,
                )
            });

            // Selection: a block per spanned logical line, split across visual rows.
            let mut selections = Vec::new();
            if !selected.is_empty() {
                let (s_line, s_col) = line_col(&ranges, selected.start);
                let (e_line, e_col) = line_col(&ranges, selected.end);
                let right = bounds.left() + wrap_width;
                // Indexing `wrapped`/`ranges`/`tops` by the same logical-line number.
                #[allow(clippy::needless_range_loop)]
                for line in s_line..=e_line {
                    let Some(wl) = wrapped.get(line) else {
                        continue;
                    };
                    let line_len = ranges.get(line).map(Range::len).unwrap_or(0);
                    let start_col = if line == s_line { s_col } else { 0 };
                    let end_col = if line == e_line { e_col } else { line_len };
                    let p0 = wl
                        .position_for_index(start_col, line_height)
                        .unwrap_or(point(px(0.), px(0.)));
                    let p1 = wl
                        .position_for_index(end_col, line_height)
                        .unwrap_or(point(px(0.), px(0.)));
                    let line_top = bounds.top() + tops[line];
                    let mut quad = |x0: Pixels, x1: Pixels, row_y: Pixels| {
                        selections.push(fill(
                            Bounds::from_corners(
                                point(x0, line_top + row_y),
                                point(x1, line_top + row_y + line_height),
                            ),
                            selection_color,
                        ));
                    };
                    if p0.y == p1.y {
                        quad(bounds.left() + p0.x, bounds.left() + p1.x, p0.y);
                    } else {
                        quad(bounds.left() + p0.x, right, p0.y);
                        let mut y = p0.y + line_height;
                        while y < p1.y {
                            quad(bounds.left(), right, y);
                            y += line_height;
                        }
                        quad(bounds.left(), bounds.left() + p1.x, p1.y);
                    }
                }
            }

            return PrepaintState {
                lines: Vec::new(),
                wrapped,
                tops,
                line_height,
                cursor: caret,
                selections,
            };
        }

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
            wrapped: Vec::new(),
            tops: Vec::new(),
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
        let wrapped = std::mem::take(&mut prepaint.wrapped);
        let tops = std::mem::take(&mut prepaint.tops);
        if wrapped.is_empty() {
            for (i, line) in lines.iter().enumerate() {
                let origin = point(bounds.left(), bounds.top() + line_height * i as f32);
                let _ = line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx);
            }
        } else {
            for (i, line) in wrapped.iter().enumerate() {
                let origin = point(bounds.left(), bounds.top() + tops[i]);
                let _ = line.paint(origin, line_height, gpui::TextAlign::Left, None, window, cx);
            }
        }

        // Caret-follow autoscroll: after an edit or caret move, nudge the scroll
        // offset so the caret sits inside the viewport. Gated on `scroll_to_cursor`
        // (set by those mutations) so a wheel-scroll away from the caret isn't undone
        // every frame. The caret quad is already in window space (scroll-translated),
        // so we compare it against the live viewport bounds. set_offset lands next
        // frame, hence the refresh; the flag is cleared below so it can't loop.
        if self.editor.read(cx).scroll_to_cursor {
            if let Some(caret) = prepaint.cursor.as_ref() {
                let scroll = self.editor.read(cx).scroll_handle.clone();
                let view = scroll.bounds();
                if view.size.height > px(0.) {
                    let mut offset = scroll.offset();
                    let start = offset.y;
                    if caret.bounds.top() < view.top() {
                        offset.y += view.top() - caret.bounds.top();
                    } else if caret.bounds.bottom() > view.bottom() {
                        offset.y -= caret.bounds.bottom() - view.bottom();
                    }
                    // Never reveal blank space above the first line.
                    offset.y = offset.y.min(px(0.));
                    if offset.y != start {
                        scroll.set_offset(offset);
                        window.refresh();
                    }
                }
            }
        }

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.editor.update(cx, |editor, _| {
            editor.last_bounds = Some(bounds);
            editor.last_lines = lines;
            editor.last_wrapped = wrapped;
            editor.last_line_height = line_height;
            editor.scroll_to_cursor = false;
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

/// Total visual rows `content` occupies when soft-wrapped to `wrap_width`: the sum
/// over logical lines of (wrap boundaries + 1). Used to size the soft-wrap element's
/// height. Color is irrelevant here — only the wrap geometry matters.
fn visual_row_count(
    content: &str,
    wrap_width: Pixels,
    _line_height: Pixels,
    window: &mut Window,
) -> usize {
    if content.is_empty() {
        return 1;
    }
    let text_style = window.text_style();
    let font = text_style.font();
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let color = text_style.color;
    let runs = [text_run(content.len(), &font, color)];
    let wrapped = window
        .text_system()
        .shape_text(
            SharedString::from(content.to_string()),
            font_size,
            &runs,
            Some(wrap_width),
            None,
        )
        .unwrap_or_default();
    wrapped
        .iter()
        .map(|wl| wl.wrap_boundaries().len() + 1)
        .sum::<usize>()
        .max(1)
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
        // Suppressed on a prose surface (see [`Self::gutter`]).
        let gutter = self.gutter.then(|| {
            div()
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
                }))
        });

        let scroll_area = div()
            .id("code-scroll")
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll_handle)
            .py_2()
            .child(
                div()
                    .flex()
                    .items_start()
                    .when_some(gutter, |row, g| row.child(g))
                    .child(div().flex_1().pl_3().child(CodeElement {
                        editor: cx.entity(),
                    })),
            );

        // Dim placeholder, shown only while the buffer is empty. An overlay (the
        // frame is `relative`) aligned to where the first glyph paints, so it never
        // perturbs layout or the input handler.
        let placeholder = (self.content.is_empty() && !self.placeholder.is_empty()).then(|| {
            let left = if self.gutter { px(60.) } else { px(12.) };
            div()
                .absolute()
                .top(px(8.))
                .left(left)
                .text_color(theme.text_dim)
                .child(self.placeholder.clone())
        });

        let focused = self.focus_handle.is_focused(window);

        // Completion popup, caret-anchored: `floating` drops it just below the
        // word being completed (in window space, via the cached caret geometry)
        // and snaps it inside the viewport, escaping the scroll container's clip.
        let popup = self.completion_anchor().and_then(|at| {
            let c = self.completion.as_ref()?;
            // Candidate list (left column). Each row: kind badge · label · dim detail.
            let list = div()
                .flex()
                .flex_col()
                .w(px(236.))
                .flex_none()
                .py_1()
                .children(c.candidates.iter().take(8).enumerate().map(|(i, item)| {
                    let badge = item.kind.badge(theme);
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px_2()
                        .py_0p5()
                        .text_size(px(12.))
                        .text_color(theme.text)
                        .when(i == c.selected, |d| d.bg(theme.bg_selected))
                        .when_some(badge, |d, (glyph, color)| {
                            d.child(
                                div()
                                    .flex_none()
                                    .size(px(15.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(3.))
                                    .bg(color.opacity(0.18))
                                    .text_size(px(9.))
                                    .text_color(color)
                                    .child(SharedString::from(glyph)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .child(item.label.clone()),
                        )
                        .when_some(item.detail.clone(), |d, detail| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.5))
                                    .text_color(theme.text_faint)
                                    .child(detail),
                            )
                        })
                }));
            // Doc side-panel for the highlighted item (detail header + guide line).
            let doc = c
                .candidates
                .get(c.selected)
                .filter(|s| s.detail.is_some() || s.documentation.is_some())
                .map(|s| {
                    div()
                        .w(px(184.))
                        .flex_none()
                        .border_l_1()
                        .border_color(theme.border)
                        .bg(theme.bg_panel)
                        .px(px(10.))
                        .py(px(8.))
                        .when_some(s.detail.clone(), |d, detail| {
                            d.child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.accent)
                                    .child(detail),
                            )
                        })
                        .when_some(s.documentation.clone(), |d, guide| {
                            d.child(
                                div()
                                    .mt(px(6.))
                                    .text_size(px(11.))
                                    .text_color(theme.text_muted)
                                    .child(guide),
                            )
                        })
                });
            let panel = div()
                .flex()
                .items_stretch()
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_sm)
                .shadow_lg()
                .overflow_hidden()
                .child(list)
                .when_some(doc, |d, doc| d.child(doc));
            Some(floating(panel).at(at))
        });

        // A11y: a multi-line text-input node keyed off the focus handle. The
        // editor content is read by assistive technology through the platform
        // input handler registered in prepaint (`handle_input` above).
        let a11y_id = ElementId::from(&self.focus_handle);
        let a11y_label = self.a11y_label.clone();
        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.bg_app)
            .border_1()
            .border_color(if focused {
                theme.accent
            } else if self.resting_border {
                theme.border
            } else {
                gpui::transparent_black()
            })
            .rounded(self.corner_radius.unwrap_or(theme.radius))
            .overflow_hidden()
            .key_context("CodeEditor")
            .id(a11y_id)
            .role(Role::MultilineTextInput)
            .aria_label(a11y_label)
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
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_next_occurrence))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::soft_newline))
            .on_action(cx.listener(Self::insert_tab))
            .on_action(cx.listener(Self::run))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            // Placeholder first so the (transparent) scroll area paints over it and
            // still wins mouse hits — the empty editor stays clickable to focus.
            .children(placeholder)
            .child(scroll_area)
            .children(popup)
    }
}
