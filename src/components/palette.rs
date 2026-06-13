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
    FocusHandle, Focusable, KeyBinding, ScrollStrategy, SharedString, UniformListScrollHandle,
    Window,
};

use crate::components::fuzzy::{fuzzy_match, highlighted_label};
use crate::components::text_input::{TextInput, TextInputEvent};
use crate::scrim::ScrimDismiss;
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

    /// Prefill the field's text (e.g. a prompt that edits an existing value, so a
    /// small tweak is one keystroke). In a command palette this also re-filters.
    pub fn set_query(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_content(text, cx));
        self.refilter(cx);
        cx.notify();
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
        let border = theme.border;
        let bg_elevated = theme.bg_elevated;
        let font_family = theme.font_family.clone();
        let font_base = theme.font_size;
        let font_lg = theme.font_size_lg();
        let hint_size = theme.font_size_xs();

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
                                        .text_size(hint_size)
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
            .border_color(border)
            .text_size(font_lg)
            .child(self.input.clone());

        let body = if self.prompt {
            // Prompt mode is input-only — no list, no empty-state text.
            div().into_any_element()
        } else if count == 0 {
            div()
                .p(px(22.))
                .text_center()
                .text_size(font_base)
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
            .font_family(font_family)
            .text_size(font_base)
            .bg(bg_elevated)
            .border_1()
            .border_color(border)
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
            // A backdrop click dismisses — but not the click that ends a window
            // drag begun on the scrim (see `ScrimDismiss`).
            .on_scrim_dismiss({
                let this = cx.entity().downgrade();
                move |_, cx| {
                    this.update(cx, |_, cx| cx.emit(PaletteEvent::Dismiss)).ok();
                }
            })
            .child(panel)
    }
}
