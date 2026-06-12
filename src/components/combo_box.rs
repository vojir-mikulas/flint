//! `ComboBox` — a searchable single-select dropdown: a `Select`-style trigger
//! (the current value + a disclosure chevron) that opens an anchored popover with
//! an embedded search field over a fuzzy-filtered list of options. The searchable
//! sibling of [`Select`](super::select::Select), for long lists (themes, installed
//! font families) where scanning a flat menu is painful.
//!
//! Generic and domain-free: the owner hands it a list of option labels and the
//! index of the current one, and reacts to [`ComboBoxEvent::Select`] with the
//! chosen label — the combo box knows nothing about what an option *means*.
//!
//! Stateful (held in an `Entity`) because it owns the embedded search field, the
//! fuzzy filter, keyboard navigation, and its own open flag — the owner only
//! [`open`](ComboBox::open)/[`toggle`](ComboBox::toggle)s it and feeds it options.
//! Call [`ComboBox::bind_keys`] once at startup for ↑/↓ navigation. Shares the
//! fuzzy machinery with [`Palette`](super::palette::Palette) and
//! [`Switcher`](super::switcher::Switcher).

use gpui::{
    actions, canvas, div, point, prelude::*, px, App, Bounds, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, KeyBinding, Pixels, SharedString, Window,
};

use crate::components::floating::floating;
use crate::components::fuzzy::{fuzzy_match, highlighted_label};
use crate::components::text_input::{TextInput, TextInputEvent};
use crate::theme::ActiveTheme;

actions!(flint_combo_box, [SelectNext, SelectPrev]);

/// What the owner subscribes to via `cx.subscribe`.
#[derive(Clone, Debug)]
pub enum ComboBoxEvent {
    /// The user chose this option label (Enter or click). The owner maps the
    /// label back to whatever it means (a theme name, a font family, …).
    Select(SharedString),
    /// The popover was dismissed (Escape or an outside click) without a choice.
    Dismiss,
}

/// A filtered row: the option's index in `options`, plus the matched byte offsets
/// in its label for highlighting.
struct Filtered {
    option: usize,
    positions: Vec<usize>,
}

pub struct ComboBox {
    id: SharedString,
    focus_handle: FocusHandle,
    input: Entity<TextInput>,
    options: Vec<SharedString>,
    /// The currently-applied option: drawn with a check and shown in the trigger.
    /// `None` shows the placeholder (no selection).
    current: Option<usize>,
    filtered: Vec<Filtered>,
    /// Keyboard cursor over the `filtered` list (the highlighted row).
    cursor: usize,
    open: bool,
    /// Focus the search field on the next paint (set when opening).
    needs_focus: bool,
    /// Trigger text shown when `current` is `None`.
    placeholder: SharedString,
}

impl ComboBox {
    pub fn new(id: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextInput::new(cx)
                .bare()
                .tab_stop(false)
                .with_placeholder("Search…")
        });

        cx.subscribe(&input, |this, _, event: &TextInputEvent, cx| match event {
            TextInputEvent::Change => {
                this.refilter(cx);
                cx.notify();
            }
            TextInputEvent::Submit => this.activate(this.cursor, cx),
            TextInputEvent::Cancel => this.dismiss(cx),
        })
        .detach();

        Self {
            id: id.into(),
            focus_handle: cx.focus_handle(),
            input,
            options: Vec::new(),
            current: None,
            filtered: Vec::new(),
            cursor: 0,
            open: false,
            needs_focus: false,
            placeholder: "Select…".into(),
        }
    }

    /// Call once at startup. ↑/↓ (and Ctrl-P/Ctrl-N) navigate the list, scoped to
    /// the `"ComboBox"` key context. Enter/Escape ride the search field's own
    /// `Submit`/`Cancel` bindings.
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("ComboBox");
        cx.bind_keys([
            KeyBinding::new("down", SelectNext, ctx),
            KeyBinding::new("up", SelectPrev, ctx),
            KeyBinding::new("ctrl-n", SelectNext, ctx),
            KeyBinding::new("ctrl-p", SelectPrev, ctx),
        ]);
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Text shown in the trigger when nothing is selected.
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    /// Placeholder shown inside the search field (defaults to "Search…").
    pub fn set_search_placeholder(
        &mut self,
        text: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.input
            .update(cx, |input, cx| input.set_placeholder(text, cx));
    }

    /// Replace the option list and the current selection, then re-filter against
    /// the live query. `current` is the index of the applied option (the one that
    /// reads in the trigger and carries the check), or `None` for no selection.
    pub fn set_options(
        &mut self,
        options: Vec<SharedString>,
        current: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        self.options = options;
        self.current = current.filter(|&ix| ix < self.options.len());
        self.refilter(cx);
        cx.notify();
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.needs_focus = true;
        // Each open starts from a clean query.
        self.input.update(cx, |input, cx| input.set_content("", cx));
        self.refilter(cx);
        let handle = self.input.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else {
            self.open(window, cx);
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.emit(ComboBoxEvent::Dismiss);
        cx.notify();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).content();
        let mut filtered = Vec::new();
        for (ix, label) in self.options.iter().enumerate() {
            if let Some((_score, positions)) = fuzzy_match(&query, label) {
                filtered.push(Filtered {
                    option: ix,
                    positions,
                });
            }
        }
        self.filtered = filtered;
        // Park the cursor on the current selection when it survived the filter
        // (so opening with an empty query highlights what's already chosen);
        // otherwise fall back to the first match.
        self.cursor = self
            .current
            .and_then(|cur| self.filtered.iter().position(|f| f.option == cur))
            .unwrap_or(0);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.filtered.len();
        if len != 0 {
            self.cursor = (self.cursor + 1) % len;
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.filtered.len();
        if len != 0 {
            self.cursor = (self.cursor + len - 1) % len;
            cx.notify();
        }
    }

    fn activate(&mut self, cursor: usize, cx: &mut Context<Self>) {
        if let Some(f) = self.filtered.get(cursor) {
            // The chosen row becomes the current selection straight away, so the
            // trigger reflects the pick without waiting for the owner to feed the
            // selection back via `set_options`.
            self.current = Some(f.option);
            let label = self.options[f.option].clone();
            self.open = false;
            cx.emit(ComboBoxEvent::Select(label));
            cx.notify();
        }
    }
}

impl Focusable for ComboBox {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<ComboBoxEvent> for ComboBox {}

impl Render for ComboBox {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }

        let theme = cx.theme().clone();
        let open = self.open;

        // Anchor the popover to the trigger's *measured* window bounds (same canvas
        // trick as `Select`/`Switcher`), so it drops from the trigger's bottom-left.
        let bounds_state = window.use_keyed_state(
            SharedString::from(format!("{}__cb_bounds", self.id)),
            cx,
            |_, _| None::<Bounds<Pixels>>,
        );
        let trigger_bounds = *bounds_state.read(cx);
        let measure = bounds_state.clone();

        // ----- trigger ----- (mirrors `Select`'s input-pill trigger so the closed
        // control looks identical to the non-searchable dropdown it replaces.)
        let has_selection = self.current.is_some();
        let current_label = self
            .current
            .and_then(|ix| self.options.get(ix).cloned())
            .unwrap_or_else(|| self.placeholder.clone());
        let trigger = div()
            .id(self.id.clone())
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(24.))
            .px_2()
            .rounded(theme.radius)
            .bg(theme.bg_input)
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .text_size(theme.font_size)
            .font_weight(FontWeight::MEDIUM)
            .text_color(if has_selection {
                theme.accent
            } else {
                theme.text_faint
            })
            .cursor_pointer()
            .child(div().child(current_label))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_color(theme.accent)
                    .text_size(theme.font_size_micro())
                    .line_height(px(6.))
                    .child("⌃")
                    .child("⌄"),
            )
            .child(
                // Invisible overlay recording the trigger's window bounds so the
                // popover can anchor to its bottom-left. Re-renders only on change.
                canvas(
                    move |bounds, _, cx| {
                        measure.update(cx, |stored, cx| {
                            if *stored != Some(bounds) {
                                *stored = Some(bounds);
                                cx.notify();
                            }
                        });
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .when(!open, |this| {
                this.hover(|s| s.border_color(theme.border_strong))
                    .on_click(cx.listener(|this, _, window, cx| this.open(window, cx)))
            });

        // ----- popover body -----
        // Token snapshot so the row closures don't borrow `cx`.
        let text = theme.text;
        let text_faint = theme.text_faint;
        let accent = theme.accent;
        let bg_selected = theme.bg_selected;
        let bg_hover = theme.bg_hover;
        let font_base = theme.font_size;
        let font_sm = theme.font_size_sm();
        let view = cx.entity().downgrade();
        let cursor = self.cursor;
        let current = self.current;

        let rows: Vec<_> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(row_ix, f)| {
                let is_cursor = row_ix == cursor;
                let is_current = Some(f.option) == current;
                let view = view.clone();
                let check = is_current.then(|| {
                    div()
                        .flex_none()
                        .text_size(font_sm)
                        .text_color(accent)
                        .child("✓")
                });
                div()
                    .id(("combo-row", row_ix))
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_cursor, |d| d.bg(bg_selected))
                    .when(!is_cursor, |d| d.hover(move |s| s.bg(bg_hover)))
                    .child(div().flex_1().min_w_0().child(highlighted_label(
                        &self.options[f.option],
                        &f.positions,
                        text,
                        accent,
                    )))
                    .when_some(check, |this, check| this.child(check))
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.activate(row_ix, cx)).ok();
                    })
            })
            .collect();

        let body = if rows.is_empty() {
            div()
                .p(px(18.))
                .text_center()
                .text_size(font_base)
                .text_color(text_faint)
                .child("No matches")
                .into_any_element()
        } else {
            div()
                .id("combo-list")
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(6.))
                .max_h(px(320.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };

        let input_row = div()
            .flex()
            .items_center()
            .px(px(12.))
            .py(px(9.))
            .border_b_1()
            .border_color(theme.border_soft)
            .text_size(font_base)
            .child(self.input.clone());

        let panel = div()
            .id("combo-popover")
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(240.))
            .font_family(theme.font_family.clone())
            .text_size(font_base)
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(px(10.))
            .shadow_lg()
            .overflow_hidden()
            .key_context("ComboBox")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss(cx)))
            .child(input_row)
            .child(body);

        div()
            .child(trigger)
            .when(open, |this| match trigger_bounds {
                Some(b) => this.child(
                    floating(panel)
                        .at(b.bottom_left())
                        .offset(point(px(0.), px(4.))),
                ),
                None => this,
            })
    }
}
