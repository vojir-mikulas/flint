// SPDX-License-Identifier: GPL-3.0-or-later

//! `MarkdownEditor` — a prose markdown surface that renders formatting *live*,
//! Obsidian "Live Preview" style: while you edit the raw markdown, headings take
//! real sizes, `**bold**` / `*italic*` / `~~strike~~` render with real weight and
//! style, inline `` `code` `` gets a mono accent chip, fenced ```` ``` ```` blocks
//! are syntax-highlighted (via [`crate::syntax`], keyed on the fence's language),
//! `[[wikilinks]]` and `#tags` are tinted, and the syntax markers are dimmed rather
//! than removed (the buffer stays byte-for-byte the source, so nothing is ever
//! re-serialised and the caret model stays exact).
//!
//! It shares `CodeEditor`'s editing core — caret / selection / word + line / doc
//! navigation, undo-with-coalescing, mouse hit-testing, IME — but is its own type
//! so `CodeEditor` (used by RED) is never touched. The rendering is the net-new
//! part: instead of one shaped layout over the whole buffer at a single font size,
//! every logical line is shaped on its own so each can carry its own size and a set
//! of per-span styled [`TextRun`]s. Always soft-wraps; no gutter. A `[[`/`#`
//! completion popup is owner-driven: the editor detects the trigger context and
//! emits [`MarkdownEditorEvent::Completion`]; the owner pushes candidates back via
//! [`MarkdownEditor::set_completions`].

use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter,
    FocusHandle, Focusable, Font, FontStyle, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    Role, SharedString, StrikethroughStyle, Style, TextRun, UTF16Selection, Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::components::code_editor::{CompletionItem, TokenStyle};
use crate::components::floating::floating;
use crate::theme::{ActiveTheme, Theme};

actions!(
    flint_markdown_editor,
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
        DeleteWordLeft,
        DeleteWordRight,
        DocStart,
        DocEnd,
        SelectDocStart,
        SelectDocEnd,
        DeleteToLineStart,
        DeleteToLineEnd,
        DuplicateLine,
        DeleteLine,
        SelectLine,
        Outdent,
        SelectNextOccurrence,
    ]
);

const UNDO_LIMIT: usize = 200;

#[derive(Clone)]
struct EditSnapshot {
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum EditKind {
    Insert,
    Delete,
    Other,
}

/// Emitted so the owner reacts to editor-level keys without the editor knowing
/// what they mean (e.g. ⌘↵ flushes a save in VAN).
#[derive(Clone, Debug)]
pub enum MarkdownEditorEvent {
    /// ⌘↵ — the owner acts on the buffer (e.g. save).
    Run,
    /// Esc with focus in the editor — the owner can move focus elsewhere.
    Escape,
    /// The `[[`/`#` completion context changed (the popup opened or its query
    /// grew/shrank). The owner should compute candidates for `query` and push
    /// them back with [`MarkdownEditor::set_completions`]. The editor stays
    /// domain-free — it never knows what a note or a tag is.
    Completion {
        trigger: CompletionTrigger,
        query: String,
    },
    /// The pointer entered or left an inline `[[wikilink]]`. `target` is the link's
    /// destination (the text before any `|alias` or `#heading`), or `None` when the
    /// pointer left a link. `at` is the window-space point just below the link, where
    /// the owner can float a preview. Emitted only when the hovered target changes.
    LinkHover {
        target: Option<String>,
        at: Point<Pixels>,
    },
}

/// Which markup the completion popup is filling in. The editor detects this from
/// the text before the caret and reports it so the owner knows which candidate
/// set to supply; on accept the editor wraps the choice appropriately.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CompletionTrigger {
    /// A `[[wikilink]]` — the accepted candidate is wrapped to `[[choice]]`.
    Wikilink,
    /// A `#tag` — the accepted candidate replaces the typed tag text.
    Tag,
}

/// A block command offered by the `/` menu: matched against `keys`, shown as
/// `label` + `hint`, and on accept it replaces the typed `/query` with `insert`,
/// landing the caret `caret` bytes into it.
struct SlashCmd {
    keys: &'static str,
    label: &'static str,
    hint: &'static str,
    insert: &'static str,
    caret: usize,
}

/// What Enter should do on a list line (see [`MarkdownEditor::list_continuation`]).
enum ListContinuation {
    /// Continue the list: insert a newline followed by this marker prefix.
    Continue(String),
    /// Exit the list: clear the empty marker, replacing `from..to` with nothing.
    Clear { from: usize, to: usize },
}

/// A parsed list marker at the start of a line: its byte length and the marker
/// prefix that should begin the next item (ordered numbers are incremented).
struct ListMarker {
    len: usize,
    next: String,
}

/// Parse a leading markdown list marker — unordered (`-`/`*`/`+`), todo
/// (`- [ ]`/`- [x]`), ordered (`1.`/`1)`), or blockquote (`> `) — preserving the
/// original indentation. Returns `None` when the line isn't a list item.
fn parse_list_marker(line: &str) -> Option<ListMarker> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    let first = rest.chars().next()?;

    // Unordered bullet, optionally a todo checkbox.
    if matches!(first, '-' | '*' | '+') {
        let after = &rest[1..];
        let body = after.strip_prefix(' ')?;
        let lower = body.to_ascii_lowercase();
        if lower.starts_with("[ ] ") || lower.starts_with("[x] ") {
            return Some(ListMarker {
                len: indent_len + 2 + 4, // "- " + "[ ] "
                next: format!("{indent}{first} [ ] "),
            });
        }
        return Some(ListMarker {
            len: indent_len + 2, // "- "
            next: format!("{indent}{first} "),
        });
    }

    // Blockquote.
    if first == '>' && rest.starts_with("> ") {
        return Some(ListMarker {
            len: indent_len + 2,
            next: format!("{indent}> "),
        });
    }

    // Ordered list: digits then `.` or `)` then a space.
    if first.is_ascii_digit() {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        let after = &rest[digits.len()..];
        let delim = after.chars().next()?;
        if matches!(delim, '.' | ')') && after[1..].starts_with(' ') {
            let n: u64 = digits.parse().ok()?;
            return Some(ListMarker {
                len: indent_len + digits.len() + 2, // digits + delim + space
                next: format!("{indent}{}{delim} ", n + 1),
            });
        }
    }

    None
}

const SLASH: &[SlashCmd] = &[
    SlashCmd {
        keys: "heading title h1",
        label: "Heading 1",
        hint: "#",
        insert: "# ",
        caret: 2,
    },
    SlashCmd {
        keys: "heading h2 subtitle",
        label: "Heading 2",
        hint: "##",
        insert: "## ",
        caret: 3,
    },
    SlashCmd {
        keys: "heading h3",
        label: "Heading 3",
        hint: "###",
        insert: "### ",
        caret: 4,
    },
    SlashCmd {
        keys: "todo task checkbox",
        label: "To-do",
        hint: "- [ ]",
        insert: "- [ ] ",
        caret: 6,
    },
    SlashCmd {
        keys: "bullet list unordered",
        label: "Bullet list",
        hint: "-",
        insert: "- ",
        caret: 2,
    },
    SlashCmd {
        keys: "number ordered list",
        label: "Numbered list",
        hint: "1.",
        insert: "1. ",
        caret: 3,
    },
    SlashCmd {
        keys: "quote blockquote callout",
        label: "Quote",
        hint: ">",
        insert: "> ",
        caret: 2,
    },
    SlashCmd {
        keys: "divider rule horizontal break",
        label: "Divider",
        hint: "---",
        insert: "---\n",
        caret: 4,
    },
    SlashCmd {
        keys: "code block fence",
        label: "Code block",
        hint: "```",
        insert: "```\n\n```",
        caret: 4,
    },
];

/// An open `/` menu: where the slash starts (replaced on accept) and which command
/// is highlighted. The query is `content[start + 1 ..= caret]`.
#[derive(Clone)]
struct Slash {
    start: usize,
    selected: usize,
}

/// An open `[[`/`#` completion popup. `anchor` is where the marker begins (the
/// popup drops below it); `query_start` is where the typed query begins (just
/// after the marker), the span replaced on accept; `selected` is the highlighted
/// row into the owner-supplied candidate list.
#[derive(Clone)]
struct Completion {
    trigger: CompletionTrigger,
    anchor: usize,
    query_start: usize,
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

// --- markdown decoration model -------------------------------------------------

/// A semantic colour the editor maps to a theme colour at paint time.
#[derive(Clone, Copy, PartialEq)]
enum SemColor {
    Text,
    Faint,
    Muted,
    Link,
    Tag,
    Code,
    /// Inline `` `code` `` text — tinted with the accent so it reads as a chip.
    Accent,
    // Fenced-block syntax-highlight classes (see [`crate::syntax`]).
    Keyword,
    Str,
    Num,
    Func,
    Operator,
}

impl SemColor {
    fn color(self, t: &Theme) -> Hsla {
        match self {
            SemColor::Text => t.text,
            SemColor::Faint => t.text_faint,
            SemColor::Muted => t.text_muted,
            SemColor::Link => t.blue,
            SemColor::Tag => t.green,
            SemColor::Code => t.text,
            SemColor::Accent => t.accent,
            SemColor::Keyword => t.purple,
            SemColor::Str => t.green,
            SemColor::Num => t.orange,
            SemColor::Func => t.blue,
            SemColor::Operator => t.cyan,
        }
    }

    /// Map a [`crate::syntax`] token class onto a highlight colour.
    fn from_token(style: TokenStyle) -> Self {
        match style {
            TokenStyle::Keyword => SemColor::Keyword,
            TokenStyle::String => SemColor::Str,
            TokenStyle::Number => SemColor::Num,
            TokenStyle::Function => SemColor::Func,
            TokenStyle::Operator => SemColor::Operator,
            TokenStyle::Comment => SemColor::Faint,
            TokenStyle::Identifier => SemColor::Code,
        }
    }
}

/// Inline run attributes for a byte span within a line.
#[derive(Clone, Copy)]
struct Attr {
    color: SemColor,
    bold: bool,
    italic: bool,
    strike: bool,
    mono: bool,
    /// Inline-code chip fill (a translucent accent tint). Fenced blocks get a
    /// block-level card painted behind them instead (see `code_panels`).
    accent_bg: bool,
}

impl Default for Attr {
    fn default() -> Self {
        Attr {
            color: SemColor::Text,
            bold: false,
            italic: false,
            strike: false,
            mono: false,
            accent_bg: false,
        }
    }
}

impl Attr {
    fn faint() -> Self {
        Attr {
            color: SemColor::Faint,
            ..Default::default()
        }
    }
    fn bold(mut self) -> Self {
        self.bold = true;
        self
    }
    fn italic(mut self) -> Self {
        self.italic = true;
        self
    }
    fn strike(mut self) -> Self {
        self.strike = true;
        self
    }
    fn colored(mut self, c: SemColor) -> Self {
        self.color = c;
        self
    }
    /// Fenced code-block text: mono in the default code colour. The block fill is a
    /// card painted behind the whole fence, not a per-glyph background.
    fn code_plain(mut self) -> Self {
        self.color = SemColor::Code;
        self.mono = true;
        self
    }

    /// Inline `` `code` ``: mono, accent-tinted, over an accent chip fill — louder
    /// than the fenced card so short spans stand out in running prose.
    fn inline_code(mut self) -> Self {
        self.color = SemColor::Accent;
        self.mono = true;
        self.accent_bg = true;
        self
    }

    /// Recolour to a fenced-block syntax token, keeping the mono font + block fill.
    fn syntax(mut self, style: TokenStyle) -> Self {
        self.color = SemColor::from_token(style);
        self
    }

    /// Build a [`TextRun`] of `len` bytes from the base font + theme.
    fn run(&self, len: usize, base_font: &Font, theme: &Theme) -> TextRun {
        let mut font = base_font.clone();
        if self.bold {
            font.weight = FontWeight::BOLD;
        }
        if self.italic {
            font.style = FontStyle::Italic;
        }
        if self.mono {
            font.family = theme.mono_family.clone();
        }
        let background_color = self.accent_bg.then(|| theme.accent.opacity(0.14));
        TextRun {
            len,
            font,
            color: self.color.color(theme),
            background_color,
            underline: None,
            strikethrough: self.strike.then(|| StrikethroughStyle {
                thickness: px(1.5),
                color: Some(theme.text_muted),
            }),
        }
    }
}

/// What to do with a marker range when the line is concealed: drop it (emphasis
/// delimiters, heading hashes) or swap it for a glyph (a bullet, a checkbox).
#[derive(Clone)]
enum Sub {
    Hide,
    Glyph(SharedString, Attr),
}

/// A list line's marker kind, which picks the concealed glyph.
#[derive(Clone, Copy)]
enum ListKind {
    Bullet,
    TaskOpen,
    TaskDone,
    Ordered,
}

/// One logical line's rendering recipe: a font-size scale (headings > 1.0), the
/// base attribute for gaps, the ordered styled spans, the marker substitutions to
/// apply when concealed, and whether the line is a horizontal rule.
struct LineDecor {
    scale: f32,
    base: Attr,
    spans: Vec<(Range<usize>, Attr)>,
    subs: Vec<(Range<usize>, Sub)>,
    divider: bool,
    /// Part of a fenced code block (a ``` line or its body) — the renderer paints a
    /// rounded card behind contiguous runs of these rows.
    code_block: bool,
}

/// A line's role inside a GitHub-style markdown table, plus the table's shared
/// per-column widths (in chars, for monospace alignment) and the block's line-index
/// range (so the whole table reveals to raw source when the caret enters it).
#[derive(Clone)]
struct TableRow {
    kind: TableKind,
    widths: Rc<Vec<usize>>,
    block: Range<usize>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum TableKind {
    Header,
    Delimiter,
    Body,
}

/// The byte ranges of each cell on a pipe-delimited row, tolerating the optional
/// leading/trailing `|`: `| a | b |` and `a | b` both yield two cell ranges. The
/// ranges include any surrounding spaces (trim when reading the content).
fn split_pipe_cells(line: &str) -> Vec<Range<usize>> {
    let start = line.len() - line.trim_start().len();
    let mut s = start;
    let mut e = line.trim_end().len();
    if line[s..e].starts_with('|') {
        s += 1;
    }
    if e > s && line[s..e].ends_with('|') {
        e -= 1;
    }
    let mut cells = Vec::new();
    let mut cur = s;
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate().take(e).skip(s) {
        if *b == b'|' {
            cells.push(cur..i);
            cur = i + 1;
        }
    }
    cells.push(cur..e);
    cells
}

/// Whether a row is a table delimiter (`|---|:--:|`): every cell is non-empty and
/// made only of `-`, `:`, and spaces.
fn is_table_delimiter(line: &str) -> bool {
    if !line.contains('|') {
        return false;
    }
    let cells = split_pipe_cells(line);
    !cells.is_empty()
        && cells.iter().all(|c| {
            let t = line[c.clone()].trim();
            !t.is_empty() && t.chars().all(|ch| matches!(ch, '-' | ':'))
        })
}

/// Classify every logical line as part of a table block or not. A table is a row
/// containing `|` immediately followed by a delimiter row; its body runs until a
/// blank or non-pipe line. Fenced code is skipped so a `|` in code isn't a table.
fn table_map(content: &str) -> Vec<Option<TableRow>> {
    let lines: Vec<&str> = content.split('\n').collect();
    let n = lines.len();
    let mut out: Vec<Option<TableRow>> = vec![None; n];

    // Mark lines inside fenced code blocks (and the fence lines themselves).
    let mut in_code = vec![false; n];
    let mut fence = false;
    for (i, l) in lines.iter().enumerate() {
        if l.trim_start().starts_with("```") {
            in_code[i] = true;
            fence = !fence;
        } else {
            in_code[i] = fence;
        }
    }

    let is_row = |i: usize| !in_code[i] && lines[i].contains('|');

    let mut i = 0;
    while i < n {
        let starts_table = is_row(i)
            && !is_table_delimiter(lines[i])
            && i + 1 < n
            && is_row(i + 1)
            && is_table_delimiter(lines[i + 1]);
        if !starts_table {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i + 2;
        while end < n && is_row(end) && !lines[end].trim().is_empty() {
            end += 1;
        }

        // Column widths over the header + body rows (the delimiter is excluded).
        let mut widths: Vec<usize> = Vec::new();
        let mut measure = |idx: usize| {
            for (ci, cell) in split_pipe_cells(lines[idx]).into_iter().enumerate() {
                let w = lines[idx][cell].trim().chars().count();
                if ci >= widths.len() {
                    widths.push(w);
                } else {
                    widths[ci] = widths[ci].max(w);
                }
            }
        };
        measure(start);
        for idx in (start + 2)..end {
            measure(idx);
        }
        for w in widths.iter_mut() {
            *w = (*w).max(1);
        }

        let widths = Rc::new(widths);
        let block = start..end;
        out[start] = Some(TableRow {
            kind: TableKind::Header,
            widths: widths.clone(),
            block: block.clone(),
        });
        out[start + 1] = Some(TableRow {
            kind: TableKind::Delimiter,
            widths: widths.clone(),
            block: block.clone(),
        });
        for row in out.iter_mut().take(end).skip(start + 2) {
            *row = Some(TableRow {
                kind: TableKind::Body,
                widths: widths.clone(),
                block: block.clone(),
            });
        }
        i = end;
    }
    out
}

/// Render one concealed table row as monospace text: each cell trimmed and padded
/// to its column width, cells joined by ` │ `, with a one-space border each side.
/// Monospace + padding gives column alignment and the `│` separators for free —
/// no pixel math. The segments map padding/separators to zero-length buffer spans
/// so a click still lands near the right offset.
fn render_table_row(
    line: &str,
    widths: &[usize],
    header: bool,
    mono: &Font,
    theme: &Theme,
) -> (String, Vec<TextRun>, Vec<Segment>) {
    let content_attr = if header {
        Attr::default().bold()
    } else {
        Attr::default().colored(SemColor::Muted)
    };
    let sep_attr = Attr::faint();

    let mut display = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();

    // A run of literal display text that doesn't exist in the buffer (padding,
    // separators, borders), anchored to a buffer offset with zero length.
    let push_filler = |display: &mut String,
                       runs: &mut Vec<TextRun>,
                       segments: &mut Vec<Segment>,
                       text: &str,
                       attr: &Attr,
                       buf_at: usize| {
        let ds = display.len();
        display.push_str(text);
        runs.push(attr.run(text.len(), mono, theme));
        segments.push(Segment {
            disp_start: ds,
            disp_len: text.len(),
            buf_start: buf_at,
            buf_len: 0,
        });
    };

    let cells = split_pipe_cells(line);
    push_filler(
        &mut display,
        &mut runs,
        &mut segments,
        " ",
        &sep_attr,
        cells.first().map(|c| c.start).unwrap_or(0),
    );

    for (ci, cell) in cells.iter().enumerate() {
        if ci > 0 {
            push_filler(
                &mut display,
                &mut runs,
                &mut segments,
                " │ ",
                &sep_attr,
                cell.start,
            );
        }
        let raw = &line[cell.clone()];
        let content = raw.trim();
        // Buffer offset where the trimmed content begins.
        let content_start = cell.start + (raw.len() - raw.trim_start().len());
        if !content.is_empty() {
            let ds = display.len();
            display.push_str(content);
            runs.push(content_attr.run(content.len(), mono, theme));
            segments.push(Segment {
                disp_start: ds,
                disp_len: content.len(),
                buf_start: content_start,
                buf_len: content.len(),
            });
        }
        let pad = widths
            .get(ci)
            .copied()
            .unwrap_or(0)
            .saturating_sub(content.chars().count());
        if pad > 0 {
            let spaces = " ".repeat(pad);
            push_filler(
                &mut display,
                &mut runs,
                &mut segments,
                &spaces,
                &content_attr,
                content_start + content.len(),
            );
        }
    }
    push_filler(
        &mut display,
        &mut runs,
        &mut segments,
        " ",
        &sep_attr,
        line.len(),
    );

    (display, runs, segments)
}

/// Decorate every logical line of `content` (split on '\n', so the count matches
/// `line_ranges`). Fence state carries across lines.
fn decorate_all(content: &str) -> Vec<LineDecor> {
    let mut out = Vec::new();
    // `Some(lang)` while inside a fence; the info string drives syntax highlighting.
    let mut fence: Option<String> = None;
    for line in content.split('\n') {
        out.push(decorate_line(line, &mut fence));
    }
    out
}

fn decorate_line(line: &str, fence: &mut Option<String>) -> LineDecor {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();

    // Fenced code block: the ``` lines are concealed (so the block reads as a card,
    // its language shown as an overlay tag in `render`); the body is syntax-highlighted
    // in the fence's language, captured from the opening info string.
    if trimmed.starts_with("```") {
        if fence.is_some() {
            *fence = None; // closing fence
        } else {
            let info = trimmed.trim_start_matches('`').trim();
            *fence = Some(info.split_whitespace().next().unwrap_or("").to_string());
        }
        return fenced_marker_line(line);
    }
    if let Some(lang) = fence.as_deref() {
        return fenced_code_line(line, lang);
    }

    // Heading: # … ###### — scale the whole line, hide the leading hashes when
    // concealed (Obsidian shows just the big text).
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
        let scale = match hashes {
            1 => 1.7,
            2 => 1.42,
            3 => 1.22,
            4 => 1.1,
            5 => 1.04,
            _ => 1.0,
        };
        let marker_end = indent + hashes + 1; // hashes + the single space
        let mut spans = vec![(indent..marker_end, Attr::faint())];
        let mut subs = vec![(indent..marker_end, Sub::Hide)];
        scan_inline(
            line,
            marker_end,
            Attr::default().bold(),
            &mut spans,
            &mut subs,
        );
        return LineDecor {
            scale,
            base: Attr::default().bold(),
            spans,
            subs,
            divider: false,
            code_block: false,
        };
    }

    // Horizontal rule (`---`, `***`, `___`): a real rule when concealed, raw dashes
    // on the active line.
    if is_divider(trimmed) {
        return LineDecor {
            scale: 1.0,
            base: Attr::faint(),
            spans: vec![(0..line.len(), Attr::faint())],
            subs: Vec::new(),
            divider: true,
            code_block: false,
        };
    }

    // Blockquote: > … — dim the marker (kept visible), the rest reads muted+italic.
    if trimmed.starts_with('>') {
        let marker_end = indent + 1 + usize::from(trimmed.as_bytes().get(1) == Some(&b' '));
        let base = Attr::default().colored(SemColor::Muted).italic();
        let mut spans = vec![(indent..marker_end, Attr::faint())];
        let mut subs = Vec::new();
        scan_inline(line, marker_end, base, &mut spans, &mut subs);
        return LineDecor {
            scale: 1.0,
            base,
            spans,
            subs,
            divider: false,
            code_block: false,
        };
    }

    // List item / task — swap the marker for a glyph (•, ○, ●) when concealed; a
    // done task strikes its text. Ordered lists keep their number.
    if let Some((marker_len, kind)) = list_marker(trimmed) {
        let abs_end = indent + marker_len;
        let mut spans = vec![(indent..abs_end, Attr::faint())];
        let (glyph, base): (Option<(SharedString, Attr)>, Attr) = match kind {
            ListKind::Bullet => (
                Some(("•  ".into(), Attr::default().colored(SemColor::Muted))),
                Attr::default(),
            ),
            ListKind::TaskOpen => (
                Some(("○  ".into(), Attr::default().colored(SemColor::Muted))),
                Attr::default(),
            ),
            ListKind::TaskDone => (
                Some(("●  ".into(), Attr::default().colored(SemColor::Link))),
                Attr::default().strike().colored(SemColor::Muted),
            ),
            ListKind::Ordered => (None, Attr::default()),
        };
        let mut subs = Vec::new();
        if let Some((g, gattr)) = glyph {
            subs.push((indent..abs_end, Sub::Glyph(g, gattr)));
        }
        scan_inline(line, abs_end, base, &mut spans, &mut subs);
        return LineDecor {
            scale: 1.0,
            base,
            spans,
            subs,
            divider: false,
            code_block: false,
        };
    }

    // Plain paragraph line.
    let mut spans = Vec::new();
    let mut subs = Vec::new();
    scan_inline(line, 0, Attr::default(), &mut spans, &mut subs);
    LineDecor {
        scale: 1.0,
        base: Attr::default(),
        spans,
        subs,
        divider: false,
        code_block: false,
    }
}

/// A ``` fence line. Concealed to nothing (the raw backticks show only when the
/// cursor is on the line) so it becomes top/bottom padding for the card; the
/// language tag + copy button are drawn as overlays in `render`. Flagged
/// `code_block` so the card spans it.
fn fenced_marker_line(line: &str) -> LineDecor {
    let attr = Attr::faint();
    LineDecor {
        scale: 1.0,
        base: attr,
        spans: vec![(0..line.len(), attr)],
        subs: vec![(0..line.len(), Sub::Hide)],
        divider: false,
        code_block: true,
    }
}

/// A line inside a fenced code block: mono in the code colour, with token spans from
/// the generic [`crate::syntax`] highlighter painted on top. Gaps fall back to the
/// `base`; the block's background is a card painted behind the whole fence.
fn fenced_code_line(line: &str, lang: &str) -> LineDecor {
    let base = Attr::default().code_plain();
    let lang = (!lang.is_empty()).then_some(lang);
    let spans = crate::syntax::highlight(line, lang)
        .into_iter()
        .map(|(range, style)| (range, base.syntax(style)))
        .collect();
    LineDecor {
        scale: 1.0,
        base,
        spans,
        subs: Vec::new(),
        divider: false,
        code_block: true,
    }
}

/// Left padding for fenced code rows so code breathes inside the card instead of
/// hugging its edge. Threaded through every offset↔pixel conversion (text origin,
/// caret, selection, hit-testing, IME, popup anchors) so those stay exact; `px(0.)`
/// for every non-code row, which leaves prose untouched.
fn code_pad(is_code: bool) -> Pixels {
    if is_code {
        px(14.)
    } else {
        px(0.)
    }
}

/// A line of only `-`, `*`, or `_` (≥3 of the same), i.e. a Markdown thematic break.
fn is_divider(trimmed: &str) -> bool {
    let t = trimmed.trim_end();
    t.len() >= 3 && {
        let c = t.as_bytes()[0];
        matches!(c, b'-' | b'*' | b'_') && t.bytes().all(|b| b == c)
    }
}

/// A leading list/task marker on a trimmed line (`- `, `* `, `+ `, `1. `, `- [ ] `,
/// `- [x] `): its byte length and kind, or `None` if the line isn't a list item.
fn list_marker(trimmed: &str) -> Option<(usize, ListKind)> {
    let b = trimmed.as_bytes();
    let digits = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 && b.get(digits) == Some(&b'.') && b.get(digits + 1) == Some(&b' ') {
        return Some((digits + 2, ListKind::Ordered));
    }
    if matches!(b.first(), Some(b'-' | b'*' | b'+')) && b.get(1) == Some(&b' ') {
        if b.get(2) == Some(&b'[') && b.get(4) == Some(&b']') && b.get(5) == Some(&b' ') {
            return match b.get(3) {
                Some(b' ') => Some((6, ListKind::TaskOpen)),
                Some(b'x' | b'X') => Some((6, ListKind::TaskDone)),
                _ => Some((2, ListKind::Bullet)),
            };
        }
        return Some((2, ListKind::Bullet));
    }
    None
}

/// Scan inline markdown in `line[from..]`, pushing ordered, non-overlapping styled
/// spans into `out` and the byte ranges to hide-when-concealed into `conceal`.
/// Gaps between spans are filled with `base` by the caller. All delimiters are
/// ASCII, so byte indexing stays on char boundaries.
fn scan_inline(
    line: &str,
    from: usize,
    base: Attr,
    out: &mut Vec<(Range<usize>, Attr)>,
    subs: &mut Vec<(Range<usize>, Sub)>,
) {
    let b = line.as_bytes();
    let n = line.len();
    let mut hide = |a: usize, z: usize| subs.push((a..z, Sub::Hide));
    let mut i = from;
    while i < n {
        let c = b[i];

        // Inline code `…` — suppresses other formatting inside.
        if c == b'`' {
            if let Some(rel) = line[i + 1..].find('`') {
                let close = i + 1 + rel;
                push_span(out, i, i + 1, Attr::faint());
                push_span(out, i + 1, close, base.inline_code());
                push_span(out, close, close + 1, Attr::faint());
                hide(i, i + 1);
                hide(close, close + 1);
                i = close + 1;
                continue;
            }
        }

        // Wikilink [[…]] (optionally [[Target|alias]] — concealed shows just the
        // alias / target).
        if c == b'[' && b.get(i + 1) == Some(&b'[') {
            if let Some(rel) = line[i + 2..].find("]]") {
                let close = i + 2 + rel;
                push_span(out, i, i + 2, Attr::faint());
                push_span(out, i + 2, close, base.colored(SemColor::Link));
                push_span(out, close, close + 2, Attr::faint());
                hide(i, i + 2);
                hide(close, close + 2);
                if let Some(prel) = line[i + 2..close].find('|') {
                    hide(i + 2, i + 2 + prel + 1); // hide "Target|"
                }
                i = close + 2;
                continue;
            }
        }

        // Strikethrough ~~…~~.
        if c == b'~' && b.get(i + 1) == Some(&b'~') {
            if let Some(rel) = line[i + 2..].find("~~") {
                let close = i + 2 + rel;
                push_span(out, i, i + 2, Attr::faint());
                push_span(out, i + 2, close, base.strike());
                push_span(out, close, close + 2, Attr::faint());
                hide(i, i + 2);
                hide(close, close + 2);
                i = close + 2;
                continue;
            }
        }

        // Bold **…** / __…__.
        if c == b'*' && b.get(i + 1) == Some(&b'*') {
            if let Some(rel) = line[i + 2..].find("**") {
                let close = i + 2 + rel;
                push_span(out, i, i + 2, Attr::faint());
                push_span(out, i + 2, close, base.bold());
                push_span(out, close, close + 2, Attr::faint());
                hide(i, i + 2);
                hide(close, close + 2);
                i = close + 2;
                continue;
            }
        }
        if c == b'_' && b.get(i + 1) == Some(&b'_') && boundary_before(b, i) {
            if let Some(rel) = line[i + 2..].find("__") {
                let close = i + 2 + rel;
                push_span(out, i, i + 2, Attr::faint());
                push_span(out, i + 2, close, base.bold());
                push_span(out, close, close + 2, Attr::faint());
                hide(i, i + 2);
                hide(close, close + 2);
                i = close + 2;
                continue;
            }
        }

        // Italic *…* / _…_.
        if c == b'*' {
            if let Some(rel) = line[i + 1..].find('*') {
                let close = i + 1 + rel;
                if close > i + 1 {
                    push_span(out, i, i + 1, Attr::faint());
                    push_span(out, i + 1, close, base.italic());
                    push_span(out, close, close + 1, Attr::faint());
                    hide(i, i + 1);
                    hide(close, close + 1);
                    i = close + 1;
                    continue;
                }
            }
        }
        if c == b'_' && boundary_before(b, i) {
            if let Some(rel) = line[i + 1..].find('_') {
                let close = i + 1 + rel;
                if close > i + 1 && boundary_after(b, close) {
                    push_span(out, i, i + 1, Attr::faint());
                    push_span(out, i + 1, close, base.italic());
                    push_span(out, close, close + 1, Attr::faint());
                    hide(i, i + 1);
                    hide(close, close + 1);
                    i = close + 1;
                    continue;
                }
            }
        }

        // #tag at a token start (kept visible — it's content, not a marker).
        if c == b'#' && (i == 0 || b[i - 1] == b' ') {
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'-' | b'_' | b'/')) {
                j += 1;
            }
            if j > i + 1 {
                push_span(out, i, j, base.colored(SemColor::Tag));
                i = j;
                continue;
            }
        }

        i += 1;
    }
}

fn push_span(out: &mut Vec<(Range<usize>, Attr)>, a: usize, z: usize, attr: Attr) {
    if a < z {
        out.push((a..z, attr));
    }
}

fn boundary_before(b: &[u8], i: usize) -> bool {
    i == 0 || !b[i - 1].is_ascii_alphanumeric()
}
fn boundary_after(b: &[u8], close: usize) -> bool {
    b.get(close + 1).is_none_or(|c| !c.is_ascii_alphanumeric())
}

/// Whether a `/`-menu command matches the query (empty query matches everything).
fn slash_matches(c: &SlashCmd, q: &str) -> bool {
    q.is_empty()
        || c.keys.split(' ').any(|k| k.starts_with(q))
        || c.label.to_lowercase().contains(q)
}

// --- the editor ----------------------------------------------------------------

pub struct MarkdownEditor {
    focus_handle: FocusHandle,
    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    /// During a drag: the unit to extend by, and the word/line the drag anchored on.
    drag_unit: DragUnit,
    drag_anchor: Option<Range<usize>>,
    read_only: bool,
    placeholder: SharedString,
    desired_col: Option<usize>,
    a11y_label: SharedString,
    /// An open `/` block-command menu, or `None`.
    slash: Option<Slash>,
    /// An open `[[`/`#` completion popup, or `None`.
    completion: Option<Completion>,
    /// Candidates last pushed by the owner via [`Self::set_completions`].
    completion_items: Vec<CompletionItem>,
    /// The `(trigger, query)` we last emitted a [`MarkdownEditorEvent::Completion`]
    /// for, so caret-only moves don't re-emit and the owner isn't queried twice for
    /// the same context.
    completion_emitted: Option<(CompletionTrigger, String)>,
    /// The `[[wikilink]]` target the pointer currently sits on, so a
    /// [`MarkdownEditorEvent::LinkHover`] fires only when it changes (not on every
    /// pixel of movement within the same link).
    hover_link: Option<String>,
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    last_edit: EditKind,
    last_edit_caret: usize,
    // Cached from the last paint, for hit-testing between paints.
    last_bounds: Option<Bounds<Pixels>>,
    last_rows: Vec<RowMetrics>,
    last_content_height: Pixels,
    /// Fenced code blocks from the last paint, so `render` can place the language
    /// label + copy button at each one's top (their pixel tops are only known
    /// post-layout).
    last_code_blocks: Vec<CodeBlockHit>,
    /// Index into `last_code_blocks` whose copy button just fired, for the brief
    /// "Copied!" flash; cleared by a spawned timer.
    copied_block: Option<usize>,
}

/// A fenced code block located during paint: its top edge (local to the editor), its
/// info-string language, and the raw code it holds (fence lines excluded) — for the
/// overlay header (language tag + copy button).
#[derive(Clone)]
struct CodeBlockHit {
    top: Pixels,
    lang: String,
    code: String,
}

/// Per logical line geometry cached from the last paint.
struct RowMetrics {
    wrapped: Option<WrappedLine>,
    range: Range<usize>,
    top: Pixels,
    /// The line-height this line was shaped at (scaled for headings).
    line_height: Pixels,
    /// Display→buffer map for a concealed line; identity `[(0, len, 0)]` when the
    /// line is revealed. Used to turn a mouse hit into a buffer offset.
    segments: Vec<Segment>,
    /// A concealed thematic break (`---`): paint a rule instead of text.
    divider: bool,
    /// The row's full laid-out height (text height incl. soft-wrap, or line height).
    height: Pixels,
    /// Part of a fenced code block — drives the rounded card painted behind the fence.
    code: bool,
    /// A concealed markdown-table row — drives the bordered panel behind the table.
    table: bool,
}

impl MarkdownEditor {
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
            desired_col: None,
            a11y_label: SharedString::from("Markdown editor"),
            slash: None,
            completion: None,
            completion_items: Vec::new(),
            completion_emitted: None,
            hover_link: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit: EditKind::Other,
            last_edit_caret: 0,
            last_bounds: None,
            last_rows: Vec::new(),
            last_content_height: px(0.),
            last_code_blocks: Vec::new(),
            copied_block: None,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn a11y_label(mut self, label: impl Into<SharedString>) -> Self {
        self.a11y_label = label.into();
        self
    }

    pub fn content(&self) -> String {
        self.content.clone()
    }

    pub fn selected_text(&self) -> Option<String> {
        (!self.selected_range.is_empty())
            .then(|| self.content[self.selected_range.clone()].to_string())
    }

    pub fn set_content(&mut self, content: impl Into<String>, cx: &mut Context<Self>) {
        let content = content.into();
        if content != self.content {
            self.record_undo(EditKind::Other);
        }
        self.content = content;
        let end = self.content.len();
        self.selected_range = end..end;
        self.last_edit_caret = end;
        cx.notify();
    }

    pub fn set_read_only(&mut self, read_only: bool, cx: &mut Context<Self>) {
        self.read_only = read_only;
        cx.notify();
    }

    /// Call once at startup. Bindings are scoped to the `"MarkdownEditor"` context.
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("MarkdownEditor");
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
            KeyBinding::new("shift-enter", SoftNewline, ctx),
            KeyBinding::new("tab", InsertTab, ctx),
            KeyBinding::new("shift-tab", Outdent, ctx),
            KeyBinding::new("escape", Escape, ctx),
        ]);
        #[cfg(target_os = "macos")]
        cx.bind_keys([
            KeyBinding::new("cmd-a", SelectAll, ctx),
            // Line operations.
            KeyBinding::new("cmd-backspace", DeleteToLineStart, ctx),
            KeyBinding::new("cmd-l", SelectLine, ctx),
            KeyBinding::new("cmd-shift-d", DuplicateLine, ctx),
            KeyBinding::new("cmd-shift-k", DeleteLine, ctx),
            KeyBinding::new("cmd-d", SelectNextOccurrence, ctx),
            // Emacs-style caret bindings, the macOS text-field convention.
            KeyBinding::new("ctrl-a", Home, ctx),
            KeyBinding::new("ctrl-e", End, ctx),
            KeyBinding::new("ctrl-b", Left, ctx),
            KeyBinding::new("ctrl-f", Right, ctx),
            KeyBinding::new("ctrl-p", Up, ctx),
            KeyBinding::new("ctrl-n", Down, ctx),
            KeyBinding::new("ctrl-d", Delete, ctx),
            KeyBinding::new("ctrl-h", Backspace, ctx),
            KeyBinding::new("ctrl-k", DeleteToLineEnd, ctx),
            KeyBinding::new("cmd-c", Copy, ctx),
            KeyBinding::new("cmd-v", Paste, ctx),
            KeyBinding::new("cmd-x", Cut, ctx),
            KeyBinding::new("cmd-z", Undo, ctx),
            KeyBinding::new("cmd-shift-z", Redo, ctx),
            KeyBinding::new("cmd-enter", Run, ctx),
            KeyBinding::new("cmd-left", Home, ctx),
            KeyBinding::new("cmd-right", End, ctx),
            KeyBinding::new("cmd-shift-left", SelectHome, ctx),
            KeyBinding::new("cmd-shift-right", SelectEnd, ctx),
            KeyBinding::new("cmd-up", DocStart, ctx),
            KeyBinding::new("cmd-down", DocEnd, ctx),
            KeyBinding::new("cmd-shift-up", SelectDocStart, ctx),
            KeyBinding::new("cmd-shift-down", SelectDocEnd, ctx),
            KeyBinding::new("alt-left", WordLeft, ctx),
            KeyBinding::new("alt-right", WordRight, ctx),
            KeyBinding::new("alt-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("alt-shift-right", SelectWordRight, ctx),
            KeyBinding::new("alt-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("alt-delete", DeleteWordRight, ctx),
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
            KeyBinding::new("ctrl-left", WordLeft, ctx),
            KeyBinding::new("ctrl-right", WordRight, ctx),
            KeyBinding::new("ctrl-shift-left", SelectWordLeft, ctx),
            KeyBinding::new("ctrl-shift-right", SelectWordRight, ctx),
            KeyBinding::new("ctrl-backspace", DeleteWordLeft, ctx),
            KeyBinding::new("ctrl-delete", DeleteWordRight, ctx),
            KeyBinding::new("ctrl-home", DocStart, ctx),
            KeyBinding::new("ctrl-end", DocEnd, ctx),
            KeyBinding::new("ctrl-shift-home", SelectDocStart, ctx),
            KeyBinding::new("ctrl-shift-end", SelectDocEnd, ctx),
            // Line operations.
            KeyBinding::new("ctrl-shift-backspace", DeleteToLineStart, ctx),
            KeyBinding::new("ctrl-l", SelectLine, ctx),
            KeyBinding::new("ctrl-shift-d", DuplicateLine, ctx),
            KeyBinding::new("ctrl-shift-k", DeleteLine, ctx),
            KeyBinding::new("ctrl-d", SelectNextOccurrence, ctx),
        ]);
    }

    // --- line / offset geometry ---

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

    // --- cursor / selection primitives ---

    pub fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    pub fn set_cursor(&mut self, offset: usize, cx: &mut Context<Self>) {
        let mut o = offset.min(self.content.len());
        while o > 0 && !self.content.is_char_boundary(o) {
            o -= 1;
        }
        self.selected_range = o..o;
        self.selection_reversed = false;
        self.last_edit_caret = o;
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.slash = None;
        self.close_completion();
        cx.notify();
    }

    /// Select an explicit byte range (clamped to char boundaries), leaving the
    /// caret at its end. Lets the owner drive selection — e.g. highlighting the
    /// current in-note find match.
    pub fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        let mut start = range.start.min(self.content.len());
        let mut end = range.end.min(self.content.len());
        while start > 0 && !self.content.is_char_boundary(start) {
            start -= 1;
        }
        while end > 0 && !self.content.is_char_boundary(end) {
            end -= 1;
        }
        self.selected_range = start.min(end)..start.max(end);
        self.selection_reversed = false;
        self.last_edit_caret = self.selected_range.end;
        self.slash = None;
        self.close_completion();
        cx.notify();
    }

    /// The window-space Y of the top of the line containing `offset`, from the
    /// geometry cached at the last paint. The owner can scroll its container so this
    /// lands in view (e.g. jumping to a heading or a find match). `None` until the
    /// editor has painted.
    pub fn offset_window_y(&self, offset: usize) -> Option<Pixels> {
        let bounds = self.last_bounds?;
        let (line, _) = self.line_col(offset);
        let row = self.last_rows.get(line)?;
        Some(bounds.origin.y + row.top)
    }

    // --- selection formatting + slash menu --------------------------------

    /// Wrap the current selection in `left`/`right` (e.g. `**`…`**`), keeping the
    /// inner text selected. Used by the selection toolbar.
    fn wrap_selection(&mut self, left: &'static str, right: &'static str, cx: &mut Context<Self>) {
        if self.read_only || self.selected_range.is_empty() {
            return;
        }
        let r = self.selected_range.clone();
        self.record_undo(EditKind::Other);
        let inner = self.content[r.clone()].to_string();
        self.content = format!(
            "{}{left}{inner}{right}{}",
            &self.content[..r.start],
            &self.content[r.end..]
        );
        let start = r.start + left.len();
        let end = start + inner.len();
        self.selected_range = start..end;
        self.last_edit_caret = end;
        cx.notify();
    }

    /// Window point just above the selection's start, for the format toolbar.
    fn selection_anchor(&self) -> Option<Point<Pixels>> {
        if self.selected_range.is_empty() {
            return None;
        }
        let bounds = self.last_bounds?;
        let (line, col) = self.line_col(self.selected_range.start);
        let row = self.last_rows.get(line)?;
        let local = row
            .wrapped
            .as_ref()
            .and_then(|wl| wl.position_for_index(col, row.line_height))
            .unwrap_or(point(px(0.), px(0.)));
        Some(bounds.origin + point(local.x + code_pad(row.code), row.top + local.y - px(42.)))
    }

    /// The lowercased `/`-menu query (text after the slash up to the caret).
    fn slash_query(&self) -> String {
        match &self.slash {
            Some(s) => self
                .content
                .get(s.start + 1..self.cursor_offset())
                .unwrap_or("")
                .to_lowercase(),
            None => String::new(),
        }
    }

    /// The `/`-menu commands matching the current query.
    fn slash_items(&self) -> Vec<&'static SlashCmd> {
        let q = self.slash_query();
        SLASH.iter().filter(|c| slash_matches(c, &q)).collect()
    }

    /// Window point just below the character at `offset`, where a dropdown (the
    /// `/` menu or the `[[`/`#` completion popup) should drop. Derived from the
    /// geometry cached at the last paint, so it tracks the caret and scrolls with
    /// the text. `None` until the editor has painted.
    fn anchor_below(&self, offset: usize) -> Option<Point<Pixels>> {
        let bounds = self.last_bounds?;
        let (line, col) = self.line_col(offset);
        let row = self.last_rows.get(line)?;
        let local = row
            .wrapped
            .as_ref()
            .and_then(|wl| wl.position_for_index(col, row.line_height))
            .unwrap_or(point(px(0.), px(0.)));
        Some(
            bounds.origin
                + point(
                    local.x + code_pad(row.code),
                    row.top + local.y + row.line_height + px(4.),
                ),
        )
    }

    /// Window point just below the slash, where the menu drops.
    fn slash_anchor(&self) -> Option<Point<Pixels>> {
        self.anchor_below(self.slash.as_ref()?.start)
    }

    fn slash_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.slash_items().len();
        if len == 0 {
            return;
        }
        if let Some(s) = &mut self.slash {
            s.selected = (s.selected as isize + delta).rem_euclid(len as isize) as usize;
            cx.notify();
        }
    }

    /// Replace the typed `/query` with the highlighted command's block markup.
    fn accept_slash(&mut self, cx: &mut Context<Self>) -> bool {
        if self.read_only {
            return false;
        }
        let Some(s) = self.slash.clone() else {
            return false;
        };
        let items = self.slash_items();
        if items.is_empty() {
            self.slash = None;
            return false;
        }
        let cmd = items[s.selected.min(items.len() - 1)];
        let cursor = self.cursor_offset();
        self.record_undo(EditKind::Other);
        self.content = format!(
            "{}{}{}",
            &self.content[..s.start],
            cmd.insert,
            &self.content[cursor..]
        );
        let caret = s.start + cmd.caret;
        self.selected_range = caret..caret;
        self.last_edit_caret = caret;
        self.slash = None;
        cx.notify();
        true
    }

    /// Recompute whether a `/` menu should be open, from the word at the caret.
    fn refresh_slash(&mut self) {
        self.slash = None;
        if !self.selected_range.is_empty() {
            return;
        }
        let cursor = self.cursor_offset();
        let bytes = self.content.as_bytes();
        let mut start = cursor;
        while start > 0 && !matches!(bytes[start - 1], b'\n' | b' ') {
            start -= 1;
        }
        if bytes.get(start) != Some(&b'/') {
            return;
        }
        // The slash must begin a token (line start or after whitespace), and the
        // query must be a plain word — so `http://`, file paths etc. don't trigger.
        if !(start == 0 || matches!(bytes[start - 1], b'\n' | b' ')) {
            return;
        }
        let query = &self.content[start + 1..cursor];
        if !query.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return;
        }
        let q = query.to_lowercase();
        if SLASH.iter().any(|c| slash_matches(c, &q)) {
            self.slash = Some(Slash { start, selected: 0 });
        }
    }

    /// Push the candidates the owner computed for the live [`Completion`] context
    /// (in response to a [`MarkdownEditorEvent::Completion`]). Resets the highlight
    /// to the first row. A no-op flavour is just passing an empty list, which hides
    /// the popup until the next keystroke.
    pub fn set_completions(&mut self, items: Vec<CompletionItem>, cx: &mut Context<Self>) {
        self.completion_items = items;
        if let Some(c) = &mut self.completion {
            c.selected = 0;
        }
        cx.notify();
    }

    /// The lowercased query for the open completion (text after the `[[`/`#`).
    fn completion_query(&self) -> String {
        match &self.completion {
            Some(c) => self
                .content
                .get(c.query_start..self.cursor_offset())
                .unwrap_or("")
                .to_string(),
            None => String::new(),
        }
    }

    /// Window point just below the `[[`/`#` marker, where the popup drops.
    fn completion_anchor(&self) -> Option<Point<Pixels>> {
        self.anchor_below(self.completion.as_ref()?.anchor)
    }

    /// Whether the completion popup is actually visible — open with candidates and
    /// no active selection. Gates the keys the popup consumes so caret navigation
    /// still works when a query has no matches.
    fn completion_open(&self) -> bool {
        self.completion.is_some()
            && !self.completion_items.is_empty()
            && self.selected_range.is_empty()
    }

    fn completion_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.completion_items.len();
        if len == 0 {
            return;
        }
        if let Some(c) = &mut self.completion {
            c.selected = (c.selected as isize + delta).rem_euclid(len as isize) as usize;
            cx.notify();
        }
    }

    /// Replace the typed query with the highlighted candidate. For a `[[` link the
    /// choice is wrapped to `[[choice]]` (reusing an existing trailing `]]` if the
    /// caret already sits before one); for a `#` tag the bare tag text is inserted.
    fn accept_completion(&mut self, cx: &mut Context<Self>) -> bool {
        if self.read_only || !self.selected_range.is_empty() {
            return false;
        }
        let Some(c) = self.completion.clone() else {
            return false;
        };
        if self.completion_items.is_empty() {
            return false;
        }
        let item = self.completion_items[c.selected.min(self.completion_items.len() - 1)].clone();
        let label = item.label.to_string();
        let cursor = self.cursor_offset();
        let (insert, caret) = match c.trigger {
            CompletionTrigger::Wikilink => {
                if self.content[cursor..].starts_with("]]") {
                    (label.clone(), c.query_start + label.len() + 2)
                } else {
                    (format!("{label}]]"), c.query_start + label.len() + 2)
                }
            }
            CompletionTrigger::Tag => (label.clone(), c.query_start + label.len()),
        };
        self.record_undo(EditKind::Other);
        self.content = format!(
            "{}{}{}",
            &self.content[..c.query_start],
            insert,
            &self.content[cursor..]
        );
        self.selected_range = caret..caret;
        self.last_edit_caret = caret;
        self.close_completion();
        cx.notify();
        true
    }

    /// Clear all completion state (popup, candidates, the emit guard).
    fn close_completion(&mut self) {
        self.completion = None;
        self.completion_items.clear();
        self.completion_emitted = None;
    }

    /// Recompute the `[[`/`#` completion context from the text before the caret.
    /// When a fresh trigger/query is found, emit [`MarkdownEditorEvent::Completion`]
    /// so the owner can supply candidates. Never fires while the `/` menu is open.
    fn refresh_completion(&mut self, cx: &mut Context<Self>) {
        self.completion = None;
        if self.slash.is_some() || !self.selected_range.is_empty() {
            self.close_completion();
            return;
        }
        let Some((trigger, anchor, query_start)) = self.completion_context() else {
            self.close_completion();
            return;
        };
        self.completion = Some(Completion {
            trigger,
            anchor,
            query_start,
            selected: 0,
        });
        let query = self.completion_query();
        if self.completion_emitted.as_ref() != Some(&(trigger, query.clone())) {
            self.completion_emitted = Some((trigger, query.clone()));
            cx.emit(MarkdownEditorEvent::Completion { trigger, query });
        }
    }

    /// Detect a completion trigger ending at the caret: an unclosed `[[` (wikilink)
    /// or a `#tag` token. Returns `(trigger, anchor, query_start)` where `anchor` is
    /// the marker's first byte and `query_start` is the first byte after it.
    fn completion_context(&self) -> Option<(CompletionTrigger, usize, usize)> {
        let cursor = self.cursor_offset();
        let bytes = self.content.as_bytes();

        // Wikilink: scan back over the query (anything but a bracket or newline);
        // a `[[` immediately before it opens the link.
        let mut qs = cursor;
        while qs > 0 && !matches!(bytes[qs - 1], b'[' | b']' | b'\n') {
            qs -= 1;
        }
        if qs >= 2 && bytes[qs - 1] == b'[' && bytes[qs - 2] == b'[' {
            return Some((CompletionTrigger::Wikilink, qs - 2, qs));
        }

        // Tag: scan back over tag-body characters; a `#` that begins a token (line
        // start or after whitespace) with at least one char typed opens the tag.
        // The leading-char requirement keeps it off an empty `# ` heading.
        let is_tag = |b: u8| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'/');
        let mut ts = cursor;
        while ts > 0 && is_tag(bytes[ts - 1]) {
            ts -= 1;
        }
        if ts < cursor && ts > 0 && bytes[ts - 1] == b'#' {
            let before_ok = ts == 1 || matches!(bytes[ts - 2], b'\n' | b' ' | b'\t');
            if before_ok {
                return Some((CompletionTrigger::Tag, ts - 1, ts));
            }
        }
        None
    }

    /// One button on the selection format toolbar. Acts on mouse-down (and stops
    /// the event) so the editor's own click-to-place-caret doesn't collapse the
    /// selection first.
    fn fmt_button(
        &self,
        id: &'static str,
        label: &'static str,
        left: &'static str,
        right: &'static str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        div()
            .id(id)
            .px(px(7.))
            .h(px(26.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .text_size(px(13.))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .hover(|s| s.bg(theme.bg_hover).text_color(theme.text))
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.wrap_selection(left, right, cx);
                }),
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
        if self.completion_open() {
            self.completion_move(-1, cx);
            return;
        }
        if self.slash.is_some() {
            self.slash_move(-1, cx);
            return;
        }
        self.vertical(true, false, cx);
    }
    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if self.completion_open() {
            self.completion_move(1, cx);
            return;
        }
        if self.slash.is_some() {
            self.slash_move(1, cx);
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
        // Smart Home: jump to the first non-blank, then to column 0 on a second press.
        let cursor = self.cursor_offset();
        let (line, _) = self.line_col(cursor);
        let r = self.line_ranges()[line].clone();
        let lead = self.content[r.clone()]
            .bytes()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let first = r.start + lead;
        let target = if cursor == first { r.start } else { first };
        self.move_to(target, cx);
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

    fn prev_word_boundary(&self, offset: usize) -> usize {
        self.content
            .unicode_word_indices()
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }
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
        if self.accept_completion(cx) {
            return;
        }
        if self.accept_slash(cx) {
            return;
        }
        self.desired_col = None;
        // Markdown list affordance: Enter on a list line continues the list with the
        // next marker (incrementing an ordered list); Enter on an empty marker exits
        // the list, clearing it. Anything else falls through to a plain newline.
        if self.selected_range.is_empty() {
            match self.list_continuation() {
                Some(ListContinuation::Continue(prefix)) => {
                    self.replace_text_in_range(None, &format!("\n{prefix}"), window, cx);
                    return;
                }
                Some(ListContinuation::Clear { from, to }) => {
                    self.selected_range = from..to;
                    self.selection_reversed = false;
                    self.replace_text_in_range(None, "", window, cx);
                    return;
                }
                None => {}
            }
        }
        self.replace_text_in_range(None, "\n", window, cx);
    }

    /// Decide what Enter should do on the caret's line when it begins with a list
    /// marker: continue the list, exit it (empty marker), or nothing (plain split).
    fn list_continuation(&self) -> Option<ListContinuation> {
        let cursor = self.cursor_offset();
        let (line, _) = self.line_col(cursor);
        let range = self.line_ranges()[line].clone();
        let text = &self.content[range.clone()];
        let marker = parse_list_marker(text)?;
        let marker_end = range.start + marker.len;
        // The caret must be at or past the marker for it to count as a list line.
        if cursor < marker_end {
            return None;
        }
        // A marker with no content after it: Enter ends the list, clearing the line.
        if text[marker.len..].trim().is_empty() {
            return Some(ListContinuation::Clear {
                from: range.start,
                to: range.end,
            });
        }
        // Continue only from the line's end (the common case); a mid-line Enter
        // splits with a plain newline so it never duplicates text after the caret.
        if cursor != range.end {
            return None;
        }
        Some(ListContinuation::Continue(marker.next))
    }
    fn soft_newline(&mut self, _: &SoftNewline, window: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.replace_text_in_range(None, "\n", window, cx);
    }
    fn insert_tab(&mut self, _: &InsertTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.accept_completion(cx) {
            return;
        }
        if self.accept_slash(cx) {
            return;
        }
        self.desired_col = None;
        // A multi-line selection indents its lines; otherwise insert two spaces.
        if self.selection_spans_lines() {
            self.shift_lines(false, cx);
        } else {
            self.replace_text_in_range(None, "  ", window, cx);
        }
    }
    fn outdent(&mut self, _: &Outdent, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.shift_lines(true, cx);
    }
    fn run(&mut self, _: &Run, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(MarkdownEditorEvent::Run);
    }
    fn escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        // Esc dismisses an open completion popup or `/` menu, then collapses a
        // selection (deselect), and only with none of those is it the owner's to act on.
        if self.completion_open() {
            self.close_completion();
            cx.notify();
        } else if self.slash.take().is_some() {
            cx.notify();
        } else if !self.selected_range.is_empty() {
            let caret = self.cursor_offset();
            self.selected_range = caret..caret;
            self.selection_reversed = false;
            cx.notify();
        } else {
            cx.emit(MarkdownEditorEvent::Escape);
        }
    }

    /// Whether the selection touches more than one logical line.
    fn selection_spans_lines(&self) -> bool {
        let (s, _) = self.line_col(self.selected_range.start);
        let (e, _) = self.line_col(self.selected_range.end);
        s != e
    }

    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.cursor_offset();
        let (line, _) = self.line_col(cursor);
        let start = self.line_ranges()[line].start;
        if start < cursor {
            self.selected_range = start..cursor;
            self.selection_reversed = false;
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn delete_to_line_end(
        &mut self,
        _: &DeleteToLineEnd,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.cursor_offset();
        let (line, _) = self.line_col(cursor);
        let end = self.line_ranges()[line].end;
        if cursor < end {
            self.selected_range = cursor..end;
            self.replace_text_in_range(None, "", window, cx);
        } else if end < self.content.len() {
            // At end of line: swallow the newline, joining the next line up.
            self.selected_range = end..end + 1;
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn duplicate_line(&mut self, _: &DuplicateLine, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (line, _) = self.line_col(cursor);
        let r = self.line_ranges()[line].clone();
        let text = self.content[r.clone()].to_string();
        self.record_undo(EditKind::Other);
        self.content.insert_str(r.end, &format!("\n{text}"));
        // Keep the caret on the new copy, same column.
        let col = cursor - r.start;
        let caret = r.end + 1 + col.min(text.len());
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.last_edit_caret = caret;
        cx.notify();
    }

    fn delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.line_col(self.cursor_offset());
        let r = self.line_ranges()[line].clone();
        self.record_undo(EditKind::Other);
        // Remove the line and one adjoining newline (trailing if any, else leading).
        let (from, to) = if r.end < self.content.len() {
            (r.start, r.end + 1)
        } else if r.start > 0 {
            (r.start - 1, r.end)
        } else {
            (r.start, r.end)
        };
        self.content.replace_range(from..to, "");
        let caret = from.min(self.content.len());
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.last_edit_caret = caret;
        cx.notify();
    }

    fn select_line(&mut self, _: &SelectLine, _: &mut Window, cx: &mut Context<Self>) {
        let (line, _) = self.line_col(self.cursor_offset());
        let r = self.line_ranges()[line].clone();
        let end = (r.end + 1).min(self.content.len());
        self.selected_range = r.start..end;
        self.selection_reversed = false;
        self.slash = None;
        cx.notify();
    }

    /// Indent (or outdent) every line the selection touches by two spaces. With an
    /// empty selection it shifts the caret's line and keeps the caret on it.
    fn shift_lines(&mut self, outdent: bool, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let empty = self.selected_range.is_empty();
        let cursor = self.cursor_offset();
        let (s_line, _) = self.line_col(self.selected_range.start);
        let (e_line, _) = self.line_col(self.selected_range.end);
        let (caret_line, caret_col) = self.line_col(cursor);
        self.record_undo(EditKind::Other);

        let mut lines: Vec<String> = self.content.split('\n').map(str::to_string).collect();
        let mut caret_removed = 0usize;
        for (li, line) in lines.iter_mut().enumerate().take(e_line + 1).skip(s_line) {
            if outdent {
                let rem = line.bytes().take_while(|b| *b == b' ').take(2).count();
                line.replace_range(0..rem, "");
                if li == caret_line {
                    caret_removed = rem;
                }
            } else {
                line.insert_str(0, "  ");
            }
        }
        self.content = lines.join("\n");

        let nr = self.line_ranges();
        if empty {
            let line_len = nr[caret_line].len();
            let new_col = if outdent {
                caret_col.saturating_sub(caret_removed)
            } else {
                caret_col + 2
            };
            let caret = nr[caret_line].start + new_col.min(line_len);
            self.selected_range = caret..caret;
            self.last_edit_caret = caret;
        } else {
            self.selected_range = nr[s_line].start..nr[e_line].end;
            self.selection_reversed = false;
        }
        cx.notify();
    }
    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    /// Copy a fenced block's code to the clipboard and flash "Copied!" on its button
    /// for a moment (a spawned timer clears the flag).
    fn copy_code_block(&mut self, idx: usize, code: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(code));
        self.copied_block = Some(idx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(1300))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.copied_block == Some(idx) {
                    this.copied_block = None;
                    cx.notify();
                }
            });
        })
        .detach();
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

    fn record_undo(&mut self, kind: EditKind) {
        let caret = self.selected_range.start;
        let contiguous = match kind {
            EditKind::Insert => self.selected_range.is_empty() && caret == self.last_edit_caret,
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

    fn edit_kind(range: &Range<usize>, new_text: &str) -> EditKind {
        if new_text.is_empty() {
            EditKind::Delete
        } else if range.is_empty()
            && !new_text.contains('\n')
            && new_text.graphemes(true).count() == 1
        {
            EditKind::Insert
        } else {
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
    fn apply_snapshot(&mut self, snap: EditSnapshot) {
        self.content = snap.content;
        self.selected_range = snap.selected_range;
        self.selection_reversed = snap.selection_reversed;
        self.marked_range = None;
        self.desired_col = None;
        self.slash = None;
        self.last_edit = EditKind::Other;
        self.last_edit_caret = self.selected_range.start;
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.desired_col = None;
        self.is_selecting = true;
        let offset = self.index_for_mouse_position(event.position);
        match event.click_count {
            // Triple-click: select the line; dragging then extends line by line.
            n if n >= 3 => {
                let line = self.line_at(offset);
                self.drag_unit = DragUnit::Line;
                self.drag_anchor = Some(line.clone());
                self.selected_range = line;
                self.selection_reversed = false;
                self.slash = None;
                cx.notify();
            }
            // Double-click: select the word; dragging then extends word by word.
            2 => {
                let word = self.word_at(offset);
                self.drag_unit = DragUnit::Word;
                self.drag_anchor = Some(word.clone());
                self.selected_range = word;
                self.selection_reversed = false;
                self.slash = None;
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
    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }
    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.is_selecting {
            self.update_hover_link(event.position, cx);
            return;
        }
        // A drag is underway: suppress any link-hover preview.
        if self.hover_link.take().is_some() {
            cx.emit(MarkdownEditorEvent::LinkHover {
                target: None,
                at: event.position,
            });
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

    /// Set the selection to the union of the drag `anchor` and the `unit` under the
    /// cursor, reversed when dragging back before the anchor.
    fn extend_union(&mut self, anchor: Range<usize>, unit: Range<usize>, cx: &mut Context<Self>) {
        self.selected_range = anchor.start.min(unit.start)..anchor.end.max(unit.end);
        self.selection_reversed = unit.start < anchor.start;
        cx.notify();
    }

    /// The byte range of the line containing `offset` (excluding its newline).
    fn line_at(&self, offset: usize) -> Range<usize> {
        let (line, _) = self.line_col(offset);
        self.line_ranges()[line].clone()
    }

    /// ⌘D: with no selection, select the word under the caret; with one, jump it to
    /// the next occurrence of that text (wrapping around the document).
    fn select_next_occurrence(
        &mut self,
        _: &SelectNextOccurrence,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let word = self.word_at(self.cursor_offset());
            if !word.is_empty() {
                self.selected_range = word;
                self.selection_reversed = false;
                self.slash = None;
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
            self.slash = None;
            cx.notify();
        }
    }

    /// Recompute which inline `[[wikilink]]` the pointer sits on and, when it
    /// differs from the last, emit a [`MarkdownEditorEvent::LinkHover`] so the owner
    /// can show or hide a preview. The anchor point drops just below the hovered
    /// line at the pointer's x.
    fn update_hover_link(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let offset = self.index_for_mouse_position(position);
        let target = self.enclosing_wikilink(offset);
        if target == self.hover_link {
            return;
        }
        self.hover_link = target.clone();
        let (line, _) = self.line_col(offset);
        let drop = self
            .last_rows
            .get(line)
            .map(|r| r.line_height)
            .unwrap_or(px(20.));
        let at = match self.offset_window_y(offset) {
            Some(y) => point(position.x, y + drop + px(4.)),
            None => position,
        };
        cx.emit(MarkdownEditorEvent::LinkHover { target, at });
    }

    /// The `[[wikilink]]` target enclosing `offset`, if any — the destination text
    /// with any `|alias` or `#heading` stripped. Scans only the offset's line.
    fn enclosing_wikilink(&self, offset: usize) -> Option<String> {
        let (line, _) = self.line_col(offset);
        let range = self.line_ranges().get(line)?.clone();
        let text = self.content.get(range.clone())?;
        let rel = offset.saturating_sub(range.start);
        let bytes = text.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            if bytes[i] == b'[' && bytes[i + 1] == b'[' {
                if let Some(close) = text[i + 2..].find("]]") {
                    let inner = &text[i + 2..i + 2 + close];
                    let span_end = i + 2 + close + 2;
                    if rel >= i && rel <= span_end {
                        let target = inner
                            .split('|')
                            .next()
                            .unwrap_or(inner)
                            .split('#')
                            .next()
                            .unwrap_or(inner)
                            .trim();
                        if !target.is_empty() {
                            return Some(target.to_string());
                        }
                    }
                    i = span_end;
                    continue;
                }
            }
            i += 1;
        }
        None
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds.as_ref() else {
            return 0;
        };
        if self.last_rows.is_empty() {
            return 0;
        }
        let y = position.y - bounds.top();
        for (i, row) in self.last_rows.iter().enumerate() {
            let next_top = self
                .last_rows
                .get(i + 1)
                .map(|r| r.top)
                .unwrap_or(self.last_content_height);
            if y < next_top || i + 1 == self.last_rows.len() {
                let Some(wl) = row.wrapped.as_ref() else {
                    return row.range.start;
                };
                let local = point(position.x - bounds.left() - code_pad(row.code), y - row.top);
                let disp = wl
                    .closest_index_for_position(local, row.line_height)
                    .unwrap_or_else(|e| e);
                let col = display_to_local(&row.segments, disp);
                return (row.range.start + col).min(row.range.end);
            }
        }
        self.content.len()
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

impl EntityInputHandler for MarkdownEditor {
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
        self.record_undo(Self::edit_kind(&range, new_text));
        self.content =
            self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.last_edit_caret = caret;
        self.marked_range = None;
        self.refresh_slash();
        self.refresh_completion(cx);
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
        let row = self.last_rows.get(line)?;
        let wl = row.wrapped.as_ref()?;
        let local = wl.position_for_index(col, row.line_height)?;
        let x = bounds.left() + code_pad(row.code) + local.x;
        let y = bounds.top() + row.top + local.y;
        Some(Bounds::from_corners(
            point(x, y),
            point(x, y + row.line_height),
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

/// The custom element: shapes every logical line on its own (so each can have its
/// own font size and styled runs) and paints text, caret and selection.
struct MarkdownElement {
    editor: Entity<MarkdownEditor>,
}

struct PrepaintState {
    rows: Vec<RowMetrics>,
    content_height: Pixels,
    cursor: Option<gpui::PaintQuad>,
    selections: Vec<gpui::PaintQuad>,
    code_panels: Vec<gpui::PaintQuad>,
    table_panels: Vec<gpui::PaintQuad>,
    code_blocks: Vec<CodeBlockHit>,
    rule_color: Hsla,
}

impl IntoElement for MarkdownElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for MarkdownElement {
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
        // Reuse the previous paint's measured height; the next frame corrects it.
        let height = if editor.last_content_height > px(0.) {
            editor.last_content_height
        } else {
            line_height * line_count as f32
        };
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = height.into();
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
        let cursor_color = theme.accent;
        let selection_color = theme.bg_selected;

        let editor = self.editor.read(cx);
        let content = editor.content.clone();
        let selected = editor.selected_range.clone();
        let cursor = editor.cursor_offset();
        let ranges = editor.line_ranges();

        let text_style = window.text_style();
        let base_font = text_style.font();
        let base_font_size = text_style.font_size.to_pixels(window.rem_size());
        let base_line_height = window.line_height();
        let wrap_width = bounds.size.width;

        let decors = decorate_all(&content);
        let tmap = table_map(&content);
        // The same base font, switched to the monospace family — for table cells, so
        // padding aligns columns.
        let mono_font = {
            let mut f = base_font.clone();
            f.family = theme.mono_family.clone();
            f
        };

        // Shape each logical line on its own, at its own scaled size. A line is
        // "revealed" (raw markdown, markers dimmed) when the selection touches it —
        // so the row you're editing shows its source; every other row is formatted.
        let mut rows: Vec<RowMetrics> = Vec::with_capacity(ranges.len());
        let mut acc = px(0.);
        for (i, r) in ranges.iter().enumerate() {
            let decor = &decors[i];
            let line_height = base_line_height * decor.scale;
            let font_size = base_font_size * decor.scale;
            let line_text = &content[r.start..r.end];
            // A table reveals as a whole block (so editing shows raw pipes); every
            // other line reveals only when the selection touches it.
            let reveal = match tmap[i].as_ref() {
                Some(t) => {
                    let bstart = ranges[t.block.start].start;
                    let bend = ranges[t.block.end - 1].end;
                    selected.start <= bend && bstart <= selected.end
                }
                None => selected.start <= r.end && r.start <= selected.end,
            };
            let table = tmap[i].as_ref().filter(|_| !reveal);
            // A concealed `---` paints as a rule (empty text); the active line shows
            // its raw dashes like any other source.
            let is_divider = decor.divider && !reveal;
            // A collapsed (near-zero-height) row: the table delimiter, which the
            // bordered panel renders implicitly.
            let mut collapse = false;
            let (display, runs, segments) = if is_divider {
                (String::new(), Vec::new(), Vec::new())
            } else if let Some(t) = table {
                match t.kind {
                    TableKind::Delimiter => {
                        collapse = true;
                        (String::new(), Vec::new(), Vec::new())
                    }
                    TableKind::Header => {
                        render_table_row(line_text, &t.widths, true, &mono_font, &theme)
                    }
                    TableKind::Body => {
                        render_table_row(line_text, &t.widths, false, &mono_font, &theme)
                    }
                }
            } else {
                render_line(line_text, decor, reveal, &base_font, &theme)
            };
            let is_table_row = table.is_some();

            // Give headings breathing room above (like a real document), scaled to
            // the heading level. The gap is empty space owned by the previous row,
            // so caret/selection/hit-testing need no special handling.
            if i > 0 && decor.scale > 1.0 {
                acc += base_line_height * ((decor.scale - 1.0) * 0.9 + 0.15);
            }

            // Code rows are inset on both sides, so they wrap at a narrower width.
            // Table rows never wrap — wrapping would break column alignment.
            let ww = wrap_width - code_pad(decor.code_block) * 2.;
            let wrap = if is_table_row { None } else { Some(ww) };
            let wrapped = if display.is_empty() {
                None
            } else {
                window
                    .text_system()
                    .shape_text(SharedString::from(display), font_size, &runs, wrap, None)
                    .ok()
                    .and_then(|mut v| (!v.is_empty()).then(|| v.remove(0)))
            };
            let height = if collapse {
                px(7.)
            } else {
                wrapped
                    .as_ref()
                    .map(|wl| wl.size(line_height).height)
                    .unwrap_or(line_height)
            };
            rows.push(RowMetrics {
                wrapped,
                range: r.clone(),
                top: acc,
                line_height,
                segments,
                divider: is_divider,
                height,
                code: decor.code_block,
                table: is_table_row,
            });
            acc += height;
        }
        let content_height = acc;

        // A rounded card behind each contiguous run of code-block rows (the fence
        // lines plus their body), so fenced blocks read as a panel rather than raw
        // ``` text. Painted before text + selection so both sit on top.
        let mut code_panels = Vec::new();
        let mut code_blocks = Vec::new();
        let mut ci = 0;
        while ci < rows.len() {
            if !rows[ci].code {
                ci += 1;
                continue;
            }
            let start = ci;
            while ci < rows.len() && rows[ci].code {
                ci += 1;
            }
            let top = bounds.top() + rows[start].top;
            let last = &rows[ci - 1];
            let bottom = bounds.top() + last.top + last.height;
            code_panels.push(gpui::quad(
                Bounds::from_corners(
                    point(bounds.left(), top),
                    point(bounds.left() + wrap_width, bottom),
                ),
                px(10.),
                theme.bg_panel,
                px(1.),
                theme.border,
                gpui::BorderStyle::Solid,
            ));
            // The block's code, fence lines excluded — for the copy button.
            let mut code = String::new();
            for idx in start..ci {
                let lt = &content[ranges[idx].start..ranges[idx].end];
                if lt.trim_start().starts_with("```") {
                    continue;
                }
                if !code.is_empty() {
                    code.push('\n');
                }
                code.push_str(lt);
            }
            // The info string on the opening fence is the language tag.
            let opener = &content[ranges[start].start..ranges[start].end];
            let lang = opener
                .trim_start()
                .trim_start_matches('`')
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
            code_blocks.push(CodeBlockHit {
                top: rows[start].top,
                lang,
                code,
            });
        }

        // A bordered panel behind each contiguous run of concealed table rows, so
        // the block reads as a table rather than monospace lines. Mirrors the code
        // card; the header/body cells (and the delimiter's implied rule) sit on top.
        let mut table_panels = Vec::new();
        let mut tj = 0;
        while tj < rows.len() {
            if !rows[tj].table {
                tj += 1;
                continue;
            }
            let start = tj;
            while tj < rows.len() && rows[tj].table {
                tj += 1;
            }
            let top = bounds.top() + rows[start].top;
            let last = &rows[tj - 1];
            let bottom = bounds.top() + last.top + last.height;
            table_panels.push(gpui::quad(
                Bounds::from_corners(
                    point(bounds.left(), top),
                    point(bounds.left() + wrap_width, bottom),
                ),
                px(8.),
                theme.bg_panel,
                px(1.),
                theme.border,
                gpui::BorderStyle::Solid,
            ));
        }

        // Caret.
        let (cline, ccol) = editor_line_col(&ranges, cursor);
        let caret = rows.get(cline).map(|row| {
            let (x, y) = match row.wrapped.as_ref() {
                Some(wl) => {
                    let local = wl
                        .position_for_index(ccol, row.line_height)
                        .unwrap_or(point(px(0.), px(0.)));
                    (local.x, local.y)
                }
                None => (px(0.), px(0.)),
            };
            fill(
                Bounds::new(
                    point(
                        bounds.left() + code_pad(row.code) + x,
                        bounds.top() + row.top + y,
                    ),
                    size(px(2.), row.line_height),
                ),
                cursor_color,
            )
        });

        // Selection: a block per spanned logical line, split across visual rows.
        let mut selections = Vec::new();
        if !selected.is_empty() {
            let (s_line, s_col) = editor_line_col(&ranges, selected.start);
            let (e_line, e_col) = editor_line_col(&ranges, selected.end);
            for line in s_line..=e_line {
                let Some(row) = rows.get(line) else { continue };
                let line_top = bounds.top() + row.top;
                let lh = row.line_height;
                // Code rows are inset on both sides; mirror that for the highlight.
                let pad = code_pad(row.code);
                let left = bounds.left() + pad;
                let right = bounds.left() + wrap_width - pad;
                let Some(wl) = row.wrapped.as_ref() else {
                    // Empty line: a thin sliver so the selection reads continuous.
                    selections.push(fill(
                        Bounds::from_corners(
                            point(left, line_top),
                            point(left + px(6.), line_top + lh),
                        ),
                        selection_color,
                    ));
                    continue;
                };
                let line_len = ranges.get(line).map(Range::len).unwrap_or(0);
                let start_col = if line == s_line { s_col } else { 0 };
                let end_col = if line == e_line { e_col } else { line_len };
                let p0 = wl
                    .position_for_index(start_col, lh)
                    .unwrap_or(point(px(0.), px(0.)));
                let p1 = wl
                    .position_for_index(end_col, lh)
                    .unwrap_or(point(px(0.), px(0.)));
                let mut quad = |x0: Pixels, x1: Pixels, row_y: Pixels| {
                    selections.push(fill(
                        Bounds::from_corners(
                            point(x0, line_top + row_y),
                            point(x1, line_top + row_y + lh),
                        ),
                        selection_color,
                    ));
                };
                if p0.y == p1.y {
                    quad(left + p0.x, left + p1.x, p0.y);
                } else {
                    quad(left + p0.x, right, p0.y);
                    let mut y = p0.y + lh;
                    while y < p1.y {
                        quad(left, right, y);
                        y += lh;
                    }
                    quad(left, left + p1.x, p1.y);
                }
            }
        }

        PrepaintState {
            rows,
            content_height,
            cursor: caret,
            selections,
            code_panels,
            table_panels,
            code_blocks,
            rule_color: theme.border_strong,
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

        for panel in prepaint.code_panels.drain(..) {
            window.paint_quad(panel);
        }

        for panel in prepaint.table_panels.drain(..) {
            window.paint_quad(panel);
        }

        for selection in prepaint.selections.drain(..) {
            window.paint_quad(selection);
        }

        let rule_color = prepaint.rule_color;
        let rows = std::mem::take(&mut prepaint.rows);
        for row in &rows {
            if row.divider {
                // A horizontal rule centred in the row, across the text width.
                let y = bounds.top() + row.top + row.line_height * 0.5;
                window.paint_quad(fill(
                    Bounds::from_corners(
                        point(bounds.left(), y),
                        point(bounds.left() + bounds.size.width, y + px(1.)),
                    ),
                    rule_color,
                ));
            } else if let Some(wl) = row.wrapped.as_ref() {
                let origin = point(bounds.left() + code_pad(row.code), bounds.top() + row.top);
                let _ = wl.paint(
                    origin,
                    row.line_height,
                    gpui::TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        let content_height = prepaint.content_height;
        let code_blocks = std::mem::take(&mut prepaint.code_blocks);
        self.editor.update(cx, |editor, _| {
            editor.last_bounds = Some(bounds);
            editor.last_rows = rows;
            editor.last_content_height = content_height;
            editor.last_code_blocks = code_blocks;
        });
    }
}

/// Full ordered, non-overlapping coverage of `[0, len)`: the decor's styled spans
/// with the gaps between them filled by the line's base attribute.
fn cover(len: usize, decor: &LineDecor) -> Vec<(Range<usize>, Attr)> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    for (span, attr) in &decor.spans {
        let s = span.start.min(len);
        let e = span.end.min(len);
        if s >= e || s < pos {
            continue;
        }
        if s > pos {
            out.push((pos..s, decor.base));
        }
        out.push((s..e, *attr));
        pos = e;
    }
    if pos < len {
        out.push((pos..len, decor.base));
    }
    out
}

/// One display run of a concealed line, mapping display bytes back to buffer
/// (line-local) bytes for hit-testing. For copied text the two lengths match
/// (1:1); for a glyph substitution they differ, so a hit anywhere in the glyph
/// clamps into the original marker's ASCII range — never a mid-char offset.
#[derive(Clone, Copy)]
struct Segment {
    disp_start: usize,
    disp_len: usize,
    buf_start: usize,
    buf_len: usize,
}

/// Render a logical line. When `reveal` is true the raw line is shown (markers
/// dimmed, identity mapping — for the line the cursor is on). Otherwise markers are
/// substituted (hidden, or swapped for a glyph) and the formatted text is shown,
/// with a display→buffer map for hit-testing.
fn render_line(
    line: &str,
    decor: &LineDecor,
    reveal: bool,
    base_font: &Font,
    theme: &Theme,
) -> (String, Vec<TextRun>, Vec<Segment>) {
    let len = line.len();
    let coverage = cover(len, decor);

    if reveal {
        let mut runs: Vec<TextRun> = coverage
            .iter()
            .map(|(range, attr)| attr.run(range.len(), base_font, theme))
            .collect();
        if runs.is_empty() {
            runs.push(decor.base.run(len, base_font, theme));
        }
        let identity = Segment {
            disp_start: 0,
            disp_len: len,
            buf_start: 0,
            buf_len: len,
        };
        return (line.to_string(), runs, vec![identity]);
    }

    let mut subs: Vec<&(Range<usize>, Sub)> = decor.subs.iter().collect();
    subs.sort_by_key(|(r, _)| r.start);

    let mut display = String::new();
    let mut runs: Vec<TextRun> = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();

    // Emit the styled buffer text for `[a, b)`, split at coverage boundaries.
    let emit = |a: usize,
                b: usize,
                display: &mut String,
                runs: &mut Vec<TextRun>,
                segments: &mut Vec<Segment>| {
        for (range, attr) in &coverage {
            let s = range.start.max(a);
            let e = range.end.min(b);
            if s < e {
                let ds = display.len();
                display.push_str(&line[s..e]);
                runs.push(attr.run(e - s, base_font, theme));
                segments.push(Segment {
                    disp_start: ds,
                    disp_len: e - s,
                    buf_start: s,
                    buf_len: e - s,
                });
            }
        }
    };

    let mut pos = 0usize;
    for (range, sub) in subs {
        if range.start < pos {
            continue; // overlapping marker, already emitted past it
        }
        emit(pos, range.start, &mut display, &mut runs, &mut segments);
        match sub {
            Sub::Hide => {}
            Sub::Glyph(glyph, attr) => {
                let ds = display.len();
                display.push_str(glyph);
                runs.push(attr.run(glyph.len(), base_font, theme));
                segments.push(Segment {
                    disp_start: ds,
                    disp_len: glyph.len(),
                    buf_start: range.start,
                    buf_len: range.end - range.start,
                });
            }
        }
        pos = range.end;
    }
    emit(pos, len, &mut display, &mut runs, &mut segments);

    (display, runs, segments)
}

/// Map a display-byte offset on a (possibly concealed) line back to its buffer
/// line-local offset, via the line's segments.
fn display_to_local(segments: &[Segment], disp: usize) -> usize {
    for seg in segments {
        if disp < seg.disp_start + seg.disp_len {
            // 1:1 for copied text; clamped into the ASCII marker for a glyph.
            return seg.buf_start + (disp - seg.disp_start).min(seg.buf_len);
        }
    }
    segments
        .last()
        .map(|s| s.buf_start + s.buf_len)
        .unwrap_or(0)
}

fn editor_line_col(ranges: &[Range<usize>], offset: usize) -> (usize, usize) {
    for (i, r) in ranges.iter().enumerate() {
        if offset <= r.end {
            return (i, offset.saturating_sub(r.start));
        }
    }
    let last = ranges.len() - 1;
    (last, ranges[last].len())
}

impl Focusable for MarkdownEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<MarkdownEditorEvent> for MarkdownEditor {}

impl Render for MarkdownEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let focused = self.focus_handle.is_focused(window);
        let placeholder = (self.content.is_empty() && !self.placeholder.is_empty()).then(|| {
            div()
                .absolute()
                .top(px(0.))
                .left(px(0.))
                .text_color(theme.text_dim)
                .child(self.placeholder.clone())
        });

        // Selection format toolbar — floats just above the selection.
        let toolbar = (focused && !self.is_selecting && self.slash.is_none())
            .then(|| self.selection_anchor())
            .flatten()
            .map(|at| {
                let row = div()
                    .id("md-toolbar")
                    .occlude()
                    .flex()
                    .items_center()
                    .gap(px(1.))
                    .p(px(3.))
                    .rounded(theme.radius_sm)
                    .bg(theme.bg_elevated)
                    .border_1()
                    .border_color(theme.border)
                    .shadow_lg()
                    .child(self.fmt_button("md-bold", "B", "**", "**", cx))
                    .child(self.fmt_button("md-italic", "i", "*", "*", cx))
                    .child(self.fmt_button("md-strike", "S", "~~", "~~", cx))
                    .child(self.fmt_button("md-code", "</>", "`", "`", cx))
                    .child(self.fmt_button("md-link", "[[ ]]", "[[", "]]", cx));
                floating(row).at(at)
            });

        // Slash command menu — drops below the slash.
        let slash_popup = self.slash_anchor().map(|at| {
            let items = self.slash_items();
            let selected = self
                .slash
                .as_ref()
                .map(|s| s.selected)
                .unwrap_or(0)
                .min(items.len().saturating_sub(1));
            let rows = items
                .into_iter()
                .enumerate()
                .map(|(i, cmd)| {
                    div()
                        .id(SharedString::from(format!("slash-{}", cmd.label)))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(5.))
                        .text_size(px(12.5))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .when(i == selected, |d| d.bg(theme.bg_selected))
                        .hover(|s| s.bg(theme.bg_hover))
                        .child(div().flex_1().child(cmd.label))
                        .child(
                            div()
                                .font_family(theme.mono_family.clone())
                                .text_size(px(11.))
                                .text_color(theme.text_faint)
                                .child(cmd.hint),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(s) = &mut this.slash {
                                    s.selected = i;
                                }
                                this.accept_slash(cx);
                            }),
                        )
                })
                .collect::<Vec<_>>();
            let list = div()
                .id("md-slash")
                .occlude()
                .min_w(px(220.))
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_sm)
                .shadow_lg()
                .py(px(4.))
                .child(div().flex().flex_col().children(rows));
            floating(list).at(at)
        });

        // `[[`/`#` completion popup — drops below the marker, owner-fed candidates.
        let completion_popup = (self.completion.is_some()
            && self.selected_range.is_empty()
            && !self.completion_items.is_empty())
        .then(|| self.completion_anchor())
        .flatten()
        .map(|at| {
            let selected = self
                .completion
                .as_ref()
                .map(|c| c.selected)
                .unwrap_or(0)
                .min(self.completion_items.len().saturating_sub(1));
            let rows = self
                .completion_items
                .iter()
                .take(12)
                .enumerate()
                .map(|(i, item)| {
                    let label = item.label.clone();
                    let detail = item.detail.clone();
                    div()
                        .id(SharedString::from(format!("md-completion-{i}")))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .py(px(5.))
                        .text_size(px(12.5))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .when(i == selected, |d| d.bg(theme.bg_selected))
                        .hover(|s| s.bg(theme.bg_hover))
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .text_ellipsis()
                                .whitespace_nowrap()
                                .child(label),
                        )
                        .when_some(detail, |d, detail| {
                            d.child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(theme.text_faint)
                                    .child(detail),
                            )
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                if let Some(c) = &mut this.completion {
                                    c.selected = i;
                                }
                                this.accept_completion(cx);
                            }),
                        )
                })
                .collect::<Vec<_>>();
            let list = div()
                .id("md-completion")
                .occlude()
                .min_w(px(240.))
                .max_w(px(360.))
                .bg(theme.bg_elevated)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_sm)
                .shadow_lg()
                .py(px(4.))
                .child(div().flex().flex_col().children(rows));
            floating(list).at(at)
        });

        // Each fenced block gets an overlay header pinned to its top edge — a subtle
        // language tag (left) and a copy button (right). Their pixel tops come from
        // the previous paint (`last_code_blocks`), like the selection toolbar's anchor.
        // The header sits in the concealed opening-fence row, above the code.
        let pad = code_pad(true);
        let code_headers = self
            .last_code_blocks
            .iter()
            .enumerate()
            .flat_map(|(i, blk)| {
                let lang = (!blk.lang.is_empty()).then(|| {
                    div()
                        .absolute()
                        .top(blk.top + px(7.))
                        .left(pad)
                        .text_size(px(9.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text_faint)
                        .child(SharedString::from(blk.lang.to_uppercase()))
                        .into_any_element()
                });

                let copied = self.copied_block == Some(i);
                let code = blk.code.clone();
                let button = div()
                    .id(SharedString::from(format!("md-copy-{i}")))
                    .occlude()
                    .absolute()
                    .top(blk.top + px(5.))
                    .right(px(8.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(theme.radius_sm)
                    .text_size(px(10.5))
                    .text_color(if copied {
                        theme.green
                    } else {
                        theme.text_faint
                    })
                    .cursor_pointer()
                    .when(!copied, |d| {
                        d.hover(|s| s.bg(theme.bg_hover).text_color(theme.text_muted))
                    })
                    .child(if copied { "Copied!" } else { "Copy" })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.copy_code_block(i, code.clone(), cx);
                        }),
                    )
                    .into_any_element();

                lang.into_iter().chain(std::iter::once(button))
            })
            .collect::<Vec<_>>();

        let a11y_id = ElementId::from(&self.focus_handle);
        let a11y_label = self.a11y_label.clone();
        div()
            .relative()
            .w_full()
            .key_context("MarkdownEditor")
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
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::soft_newline))
            .on_action(cx.listener(Self::insert_tab))
            .on_action(cx.listener(Self::outdent))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::duplicate_line))
            .on_action(cx.listener(Self::delete_line))
            .on_action(cx.listener(Self::select_line))
            .on_action(cx.listener(Self::select_next_occurrence))
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
            .children(placeholder)
            .child(MarkdownElement {
                editor: cx.entity(),
            })
            .children(code_headers)
            .children(toolbar)
            .children(slash_popup)
            .children(completion_popup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(line: &str) -> Option<String> {
        parse_list_marker(line).map(|m| m.next)
    }

    #[test]
    fn continues_unordered_bullets() {
        assert_eq!(next("- item").as_deref(), Some("- "));
        assert_eq!(next("* item").as_deref(), Some("* "));
        assert_eq!(next("+ item").as_deref(), Some("+ "));
    }

    #[test]
    fn continues_todo_unchecked() {
        assert_eq!(next("- [ ] task").as_deref(), Some("- [ ] "));
        // A checked item still continues as a fresh, unchecked one.
        assert_eq!(next("- [x] done").as_deref(), Some("- [ ] "));
        assert_eq!(next("- [X] done").as_deref(), Some("- [ ] "));
    }

    #[test]
    fn increments_ordered_lists() {
        assert_eq!(next("1. first").as_deref(), Some("2. "));
        assert_eq!(next("9. ninth").as_deref(), Some("10. "));
        assert_eq!(next("3) paren").as_deref(), Some("4) "));
    }

    #[test]
    fn preserves_indentation() {
        assert_eq!(next("  - nested").as_deref(), Some("  - "));
        assert_eq!(next("\t1. tabbed").as_deref(), Some("\t2. "));
    }

    #[test]
    fn continues_blockquotes() {
        assert_eq!(next("> quote").as_deref(), Some("> "));
    }

    #[test]
    fn ignores_non_lists() {
        assert!(parse_list_marker("plain text").is_none());
        assert!(parse_list_marker("-no space").is_none());
        assert!(parse_list_marker("1.no space").is_none());
        assert!(parse_list_marker("# heading").is_none());
        assert!(parse_list_marker("").is_none());
    }

    #[test]
    fn marker_lengths_match_continuation() {
        // The reported length must cover exactly the marker so the editor knows
        // where the content begins (drives the "empty marker exits list" case).
        let m = parse_list_marker("- [ ] x").unwrap();
        assert_eq!(&"- [ ] x"[..m.len], "- [ ] ");
        let m = parse_list_marker("12. y").unwrap();
        assert_eq!(&"12. y"[..m.len], "12. ");
    }

    #[test]
    fn split_pipe_cells_tolerates_edge_pipes() {
        let line = "| a | bb |";
        let cells = split_pipe_cells(line);
        let texts: Vec<&str> = cells.iter().map(|c| line[c.clone()].trim()).collect();
        assert_eq!(texts, vec!["a", "bb"]);

        let line2 = "a | bb | ccc";
        let cells2 = split_pipe_cells(line2);
        let texts2: Vec<&str> = cells2.iter().map(|c| line2[c.clone()].trim()).collect();
        assert_eq!(texts2, vec!["a", "bb", "ccc"]);
    }

    #[test]
    fn delimiter_row_detection() {
        assert!(is_table_delimiter("|---|:--:|"));
        assert!(is_table_delimiter(" --- | --- "));
        assert!(!is_table_delimiter("| a | b |"));
        assert!(!is_table_delimiter("no pipes here"));
    }

    #[test]
    fn table_map_marks_block_and_skips_code() {
        let content = "intro\n| h1 | h2 |\n|----|----|\n| a | b |\n| cc | d |\nafter";
        let map = table_map(content);
        assert!(map[0].is_none()); // intro
        assert_eq!(map[1].as_ref().unwrap().kind, TableKind::Header);
        assert_eq!(map[2].as_ref().unwrap().kind, TableKind::Delimiter);
        assert_eq!(map[3].as_ref().unwrap().kind, TableKind::Body);
        assert_eq!(map[4].as_ref().unwrap().kind, TableKind::Body);
        assert!(map[5].is_none()); // after
                                   // Column widths span header + body: col0 = max("h1","a","cc")=2, col1=2.
        assert_eq!(*map[1].as_ref().unwrap().widths, vec![2, 2]);
        // The whole table shares one block range.
        assert_eq!(map[1].as_ref().unwrap().block, 1..5);

        // A pipe inside a fenced code block is not a table.
        let fenced = "```\n| not | a | table |\n|---|---|---|\n```";
        assert!(table_map(fenced).iter().all(|r| r.is_none()));
    }
}
