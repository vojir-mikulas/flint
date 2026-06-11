// SPDX-License-Identifier: GPL-3.0-or-later

//! `Palette` - a command-palette overlay: a seamless search field over a
//! fuzzy-filtered, keyboard-navigable list of commands. Modelled on the editor
//! command palettes ("Execute a command…"): label on the left, an optional
//! keybinding hint on the right, ↑/↓ to move, ↵ to run, Esc to dismiss.
//!
//! Generic and domain-free, like the rest of Flint: the owner hands it a list of
//! [`PaletteItem`]s and reacts to [`PaletteEvent::Activate`] with the `id` of the
//! chosen row — the palette knows nothing about what a command *does*. Call
//! [`Palette::bind_keys`] once at startup for ↑/↓ navigation.

use gpui::{
    actions, div, prelude::*, px, uniform_list, App, Context, ElementId, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window,
};

use crate::components::text_input::{TextInput, TextInputEvent};
use crate::theme::ActiveTheme;

actions!(flint_palette, [SelectNext, SelectPrev]);

/// One selectable row. `id` is opaque to the palette and handed straight back on
/// activation, so the owner maps it to whatever action it means.
#[derive(Clone, Debug)]
pub struct PaletteItem {
    pub id: ElementId,
    /// The command name shown on the row (e.g. `"query: run"`).
    pub label: SharedString,
    /// Optional right-aligned keybinding hint (e.g. `"⌘↵"`).
    pub hint: Option<SharedString>,
}

impl PaletteItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            hint: None,
        }
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// What the owner subscribes to via `cx.subscribe`.
#[derive(Clone, Debug)]
pub enum PaletteEvent {
    /// The user chose the row with this `id` (Enter or click). Command mode.
    Activate(ElementId),
    /// The user submitted free text (Enter). Prompt mode only — see
    /// [`Palette::prompt`].
    Submit(SharedString),
    /// The user dismissed the palette (Escape or a scrim click).
    Dismiss,
}

/// A filtered row: an index into `items` plus the byte offsets in its label that
/// the query matched, for highlighting.
struct Filtered {
    item: usize,
    score: i32,
    positions: Vec<usize>,
}

pub struct Palette {
    focus_handle: FocusHandle,
    input: Entity<TextInput>,
    items: Vec<PaletteItem>,
    filtered: Vec<Filtered>,
    selected: usize,
    scroll: UniformListScrollHandle,
    /// Prompt mode: no list; Enter emits [`PaletteEvent::Submit`] with the typed
    /// text instead of activating a command. A free-text input (e.g. "go to row").
    prompt: bool,
    /// Focus the search field on the next render. Set on construction so the
    /// palette grabs focus when it's first mounted, without the owner threading a
    /// `Window` through the open path.
    needs_focus: bool,
}

const ROW_HEIGHT: gpui::Pixels = px(34.);
/// Most command rows shown before the list scrolls (caps the panel height).
const MAX_VISIBLE_ROWS: usize = 10;

impl Palette {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextInput::new(cx)
                .bare()
                .tab_stop(false)
                .with_placeholder("Search…")
        });

        // The search field drives everything: typing re-filters, Enter runs the
        // selection (or submits the text in prompt mode), Escape dismisses.
        cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| match event {
            TextInputEvent::Change => {
                this.refilter(cx);
                cx.notify();
            }
            TextInputEvent::Submit => {
                if this.prompt {
                    let text = this.input.read(cx).content();
                    cx.emit(PaletteEvent::Submit(text));
                } else {
                    this.activate_selected(cx);
                }
            }
            TextInputEvent::Cancel => cx.emit(PaletteEvent::Dismiss),
        })
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            input,
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            scroll: UniformListScrollHandle::new(),
            prompt: false,
            needs_focus: true,
        }
    }

    /// Switch to prompt mode: no command list, Enter emits
    /// [`PaletteEvent::Submit`] with the entered text. For a free-text prompt
    /// reusing the palette's overlay/focus (e.g. "go to row…").
    pub fn prompt(mut self) -> Self {
        self.prompt = true;
        self
    }

    /// Call once at startup. ↑/↓ (and Ctrl-P/Ctrl-N) navigate the list; they're
    /// scoped to the `"Palette"` key context so they never leak elsewhere. Enter
    /// and Escape ride the embedded field's existing `Submit`/`Cancel` bindings.
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("Palette");
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, ctx),
            KeyBinding::new("up", SelectPrev, ctx),
            KeyBinding::new("ctrl-n", SelectNext, ctx),
            KeyBinding::new("ctrl-p", SelectPrev, ctx),
        ]);
    }

    /// Set the placeholder shown in the empty search field.
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_placeholder(text, cx));
    }

    /// Replace the command list and re-filter against the current query.
    pub fn set_items(&mut self, items: Vec<PaletteItem>, cx: &mut Context<Self>) {
        self.items = items;
        self.refilter(cx);
        cx.notify();
    }

    /// Focus the search field so typing lands in the palette. Call right after the
    /// palette is mounted (e.g. on open).
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).content();
        let mut filtered: Vec<Filtered> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(item, it)| {
                fuzzy_match(&query, &it.label).map(|(score, positions)| Filtered {
                    item,
                    score,
                    positions,
                })
            })
            .collect();
        // Stable sort by descending score keeps the registry's order on ties (and
        // for the empty query, where every score is 0).
        filtered.sort_by_key(|f| std::cmp::Reverse(f.score));
        self.filtered = filtered;
        self.selected = 0;
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
            self.scroll
                .scroll_to_item(self.selected, ScrollStrategy::Top);
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + self.filtered.len() - 1) % self.filtered.len();
            self.scroll
                .scroll_to_item(self.selected, ScrollStrategy::Top);
            cx.notify();
        }
    }

    fn activate_selected(&mut self, cx: &mut Context<Self>) {
        if let Some(f) = self.filtered.get(self.selected) {
            let id = self.items[f.item].id.clone();
            cx.emit(PaletteEvent::Activate(id));
        }
    }

    fn activate_index(&mut self, filtered_ix: usize, cx: &mut Context<Self>) {
        if let Some(f) = self.filtered.get(filtered_ix) {
            let id = self.items[f.item].id.clone();
            cx.emit(PaletteEvent::Activate(id));
        }
    }
}

impl Focusable for Palette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        // Defer to the search field so the palette is "focused" whenever its input is.
        self.input.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<PaletteEvent> for Palette {}

impl Render for Palette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Grab focus the first time we paint, so the field is ready for typing
        // without the owner threading a `Window` through its open path.
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }

        let theme = cx.theme();
        // Token snapshot so the `'static` row closure doesn't borrow `cx`.
        let bg_selected = theme.bg_selected;
        let bg_hover = theme.bg_hover;
        let text = theme.text;
        let text_faint = theme.text_faint;
        let accent = theme.accent;
        let border_soft = theme.border_soft;
        let bg_elevated = theme.bg_elevated;

        let view = cx.entity().downgrade();
        let selected = self.selected;
        let count = self.filtered.len();

        // Snapshot the visible rows for the list closure (label + hint + match marks).
        let rows: Vec<(SharedString, Option<SharedString>, Vec<usize>)> = self
            .filtered
            .iter()
            .map(|f| {
                let it = &self.items[f.item];
                (it.label.clone(), it.hint.clone(), f.positions.clone())
            })
            .collect();
        let rows = std::rc::Rc::new(rows);

        let list = uniform_list("palette-rows", count, {
            let rows = rows.clone();
            move |range, _window, _cx| {
                let mut out = Vec::with_capacity(range.len());
                for ix in range {
                    let (label, hint, positions) = &rows[ix];
                    let is_selected = ix == selected;
                    let view = view.clone();
                    out.push(
                        div()
                            .id(ix)
                            .flex()
                            .items_center()
                            .gap(px(11.))
                            .w_full()
                            .h(ROW_HEIGHT)
                            .px(px(12.))
                            .rounded(px(7.))
                            .cursor_pointer()
                            .when(is_selected, |d| d.bg(bg_selected))
                            .when(!is_selected, |d| d.hover(move |s| s.bg(bg_hover)))
                            .child(highlighted_label(label, positions, text, accent))
                            .when_some(hint.clone(), |d, hint| {
                                d.child(
                                    div()
                                        .ml_auto()
                                        .flex_shrink_0()
                                        .text_size(px(11.))
                                        .text_color(text_faint)
                                        .child(hint),
                                )
                            })
                            .on_click(move |_, _window, cx| {
                                view.update(cx, |this, cx| this.activate_index(ix, cx)).ok();
                            }),
                    );
                }
                out
            }
        })
        .track_scroll(&self.scroll)
        .flex_1();

        // The input row: a seamless field under a hairline divider.
        let input_row = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(15.))
            .py(px(13.))
            .border_b_1()
            .border_color(border_soft)
            .text_size(px(15.))
            .child(self.input.clone());

        let body = if self.prompt {
            // Prompt mode is input-only — no list, no empty-state text.
            div().into_any_element()
        } else if count == 0 {
            div()
                .p(px(22.))
                .text_center()
                .text_size(px(13.))
                .text_color(text_faint)
                .child("No matching commands")
                .into_any_element()
        } else {
            // `uniform_list` virtualizes against a *definite* height, so size the
            // list to its rows (capped) — a content-sized parent would collapse
            // the `flex_1` list to nothing and show no commands.
            let visible = count.clamp(1, MAX_VISIBLE_ROWS);
            let body_h = px(visible as f32 * f32::from(ROW_HEIGHT) + 12.0);
            div()
                .flex()
                .flex_col()
                .p(px(6.))
                .h(body_h)
                .child(list)
                .into_any_element()
        };

        let panel = div()
            .occlude()
            .flex()
            .flex_col()
            .w(px(560.))
            .max_w(gpui::relative(0.92))
            .bg(bg_elevated)
            .border_1()
            .border_color(border_soft)
            .rounded(px(11.))
            .shadow_lg()
            .overflow_hidden()
            // ↑/↓ navigate; the field's own context still owns text editing.
            .key_context("Palette")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .child(input_row)
            .child(body);

        // Full-screen scrim; a click outside the panel dismisses.
        div()
            .id("palette-overlay")
            .absolute()
            .inset_0()
            .flex()
            // Anchor the panel to the top (sized to its content); without this the
            // default `align-items: stretch` blows the panel up to full height.
            .items_start()
            .justify_center()
            .pt(gpui::relative(0.11))
            .bg(gpui::black().opacity(0.45))
            .occlude()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismiss)),
            )
            .child(panel)
    }
}

/// Render a label with its fuzzy-matched characters emphasised. Consecutive
/// matched/unmatched runs become sibling spans in a flex row, so the matched
/// glyphs paint in `hit` (bold) over the `base` colour.
fn highlighted_label(
    label: &SharedString,
    positions: &[usize],
    base: Hsla,
    hit: Hsla,
) -> impl IntoElement {
    let mut spans: Vec<(String, bool)> = Vec::new();
    let mut current = String::new();
    let mut current_hit = false;
    for (byte, ch) in label.char_indices() {
        let is_hit = positions.contains(&byte);
        if is_hit != current_hit && !current.is_empty() {
            spans.push((std::mem::take(&mut current), current_hit));
        }
        current_hit = is_hit;
        current.push(ch);
    }
    if !current.is_empty() {
        spans.push((current, current_hit));
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .overflow_hidden()
        .whitespace_nowrap()
        .children(spans.into_iter().map(move |(segment, is_hit)| {
            div()
                .when(is_hit, |d| d.font_weight(FontWeight::SEMIBOLD))
                .text_color(if is_hit { hit } else { base })
                .child(segment)
        }))
}

/// Case-insensitive subsequence fuzzy match of `query` against `text`. Returns
/// `None` unless every (non-whitespace) query char appears in order. On a match,
/// returns a score (higher = better) and the byte offsets in `text` that matched,
/// for highlighting. An empty query matches everything with score 0 and no marks,
/// so the list shows in its natural order.
fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let needles: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(|c| c.to_lowercase())
        .collect();
    if needles.is_empty() {
        return Some((0, Vec::new()));
    }

    let haystack: Vec<(usize, char)> = text.char_indices().collect();
    let mut qi = 0;
    let mut positions = Vec::with_capacity(needles.len());
    let mut score: i32 = 0;
    let mut prev_matched_at: Option<usize> = None;

    for (ci, (byte, ch)) in haystack.iter().enumerate() {
        if qi >= needles.len() {
            break;
        }
        let lowered = ch.to_lowercase().next().unwrap_or(*ch);
        if lowered != needles[qi] {
            continue;
        }
        positions.push(*byte);
        score += 1;
        // Adjacent to the previous match — runs of consecutive hits read best.
        if prev_matched_at == Some(ci.wrapping_sub(1)) {
            score += 5;
        }
        // At a word boundary (string start, after a separator, or a camelCase hump).
        let at_boundary = ci == 0 || {
            let prev = haystack[ci - 1].1;
            !prev.is_alphanumeric() || (prev.is_lowercase() && ch.is_uppercase())
        };
        if at_boundary {
            score += 8;
        }
        // Mild bias toward earlier matches.
        score -= ci as i32 / 4;
        prev_matched_at = Some(ci);
        qi += 1;
    }

    (qi == needles.len()).then_some((score, positions))
}

#[cfg(test)]
mod tests {
    use super::fuzzy_match;

    fn score(query: &str, text: &str) -> Option<i32> {
        fuzzy_match(query, text).map(|(s, _)| s)
    }

    #[test]
    fn empty_query_matches_everything_with_no_marks() {
        let (score, marks) = fuzzy_match("", "query: run").unwrap();
        assert_eq!(score, 0);
        assert!(marks.is_empty());
    }

    #[test]
    fn requires_all_chars_in_order() {
        assert!(fuzzy_match("run", "query: run").is_some());
        assert!(fuzzy_match("nur", "query: run").is_none()); // out of order
        assert!(fuzzy_match("runs", "query: run").is_none()); // extra char
    }

    #[test]
    fn match_is_case_insensitive() {
        assert!(fuzzy_match("RUN", "query: run").is_some());
        assert!(fuzzy_match("QR", "Query: Run").is_some());
    }

    #[test]
    fn marks_point_at_matched_bytes() {
        let (_, marks) = fuzzy_match("qr", "query: run").unwrap();
        // 'q' at byte 0, the first 'r' at byte 3 ("que[r]y").
        assert_eq!(marks, vec![0, 3]);
    }

    #[test]
    fn whitespace_in_query_is_ignored() {
        assert_eq!(score("q run", "query: run"), score("qrun", "query: run"));
    }

    #[test]
    fn prefix_beats_scattered_match() {
        // "run" as a word start should outrank the scattered r…u…n in "regular unit".
        let prefix = score("run", "run query").unwrap();
        let scattered = score("run", "regular unit number").unwrap();
        assert!(
            prefix > scattered,
            "prefix {prefix} should beat scattered {scattered}"
        );
    }

    #[test]
    fn consecutive_run_beats_scattered_run() {
        // Same chars, same text length, no word-boundary help on either side:
        // the consecutive substring should win on the adjacency bonus alone.
        let consecutive = score("abc", "abcxx").unwrap();
        let scattered = score("abc", "axbxc").unwrap();
        assert!(
            consecutive > scattered,
            "consecutive {consecutive} should beat scattered {scattered}"
        );
    }
}
