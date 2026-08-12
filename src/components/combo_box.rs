//! `ComboBox` — a searchable dropdown: a `Select`-style trigger (the current
//! value + a disclosure chevron) that opens an anchored popover with an embedded
//! search field over a fuzzy-filtered list of options. The searchable sibling of
//! [`Select`](super::select::Select), for long lists (themes, installed font
//! families) where scanning a flat menu is painful.
//!
//! Single-select by default. [`set_multi`](ComboBox::set_multi) switches it to a
//! **multi-select filter**: every option carries its own check, a pick toggles
//! rather than replaces and leaves the popover open (so several can be chosen in
//! one visit), the trigger summarises the set as `First +2`, and a Clear appears
//! beside the search field once anything is on.
//!
//! Generic and domain-free: the owner hands it a list of option labels and which
//! are selected, and reacts to [`ComboBoxEvent::Select`] / [`ComboBoxEvent::Toggle`]
//! with the label — the combo box knows nothing about what an option *means*.
//!
//! Stateful (held in an `Entity`) because it owns the embedded search field, the
//! fuzzy filter, keyboard navigation, and its own open flag — the owner only
//! [`open`](ComboBox::open)/[`toggle`](ComboBox::toggle)s it and feeds it options.
//! Call [`ComboBox::bind_keys`] once at startup for ↑/↓ navigation. Shares the
//! fuzzy machinery with [`Palette`](super::palette::Palette) and
//! [`Switcher`](super::switcher::Switcher).

use gpui::{
    actions, canvas, div, point, prelude::*, px, AnyElement, App, Bounds, Context, Entity,
    EventEmitter, FocusHandle, Focusable, KeyBinding, Pixels, Role, SharedString, Window,
};

use crate::components::floating::floating;
use crate::components::fuzzy::{fuzzy_match, highlighted_label};
use crate::components::text_input::{TextInput, TextInputEvent};
use crate::theme::ActiveTheme;

actions!(flint_combo_box, [SelectNext, SelectPrev]);

/// A glyph factory, re-invoked each render so the icon re-themes with the app
/// (size/colour follow the current theme). Caller-supplied so Flint stays
/// domain-free — RED hands in its lucide SVGs, the gallery a unicode mark.
type IconFn = Box<dyn Fn(&App) -> AnyElement + 'static>;

/// A per-option leading-element factory, keyed by the option's index in the list
/// last handed to [`ComboBox::set_options`]. Re-invoked each render so it
/// re-themes with the app. Drawn before the label in the trigger (for the current
/// selection) and on every popover row — e.g. a colour swatch or engine glyph.
/// Domain-free: the combo passes only the index, the caller decides what to draw.
type LeadingFn = Box<dyn Fn(usize, &App) -> AnyElement + 'static>;

/// What the owner subscribes to via `cx.subscribe`.
#[derive(Clone, Debug)]
pub enum ComboBoxEvent {
    /// Single-select: the user chose this option label (Enter or click). The
    /// owner maps the label back to whatever it means (a theme name, a font
    /// family, …).
    Select(SharedString),
    /// Multi-select: this option label was flipped. The combo has already applied
    /// it to its own set, so `selected` says which way it went and the owner just
    /// mirrors it.
    Toggle { label: SharedString, selected: bool },
    /// Multi-select: the popover's Clear was pressed; nothing is selected now.
    Clear,
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
    /// Single-select: the currently-applied option, drawn with a check and shown
    /// in the trigger. `None` shows the placeholder (no selection). Unused in
    /// multi-select, where [`selected`](Self::selected) is the whole answer.
    current: Option<usize>,
    /// Multi-select: every applied option, in the order it was picked so the
    /// trigger's summary doesn't reshuffle under the cursor. Empty = no filter.
    selected: Vec<usize>,
    /// Whether a pick toggles (and keeps the popover open) or replaces and closes.
    multi: bool,
    filtered: Vec<Filtered>,
    /// Keyboard cursor over the `filtered` list (the highlighted row).
    cursor: usize,
    open: bool,
    /// Focus the search field on the next paint (set when opening).
    needs_focus: bool,
    /// Trigger text shown when `current` is `None`.
    placeholder: SharedString,
    /// Trigger disclosure glyph; falls back to a stacked unicode chevron.
    chevron: Option<IconFn>,
    /// Mark on the selected row; falls back to a unicode check.
    check: Option<IconFn>,
    /// Optional leading element per option (colour dot, glyph). Keyed by option
    /// index; drawn in the trigger and on each row. `None` = label only.
    leading: Option<LeadingFn>,
    /// Optional trailing element per option (a count, a shortcut hint), drawn on
    /// the popover row between the label and the check. Row-only: the trigger has
    /// no room for it. `None` = nothing after the label.
    trailing: Option<LeadingFn>,
    /// Stretch the trigger to fill its parent's width. Default `false` (the
    /// trigger sizes to its content, like a menu button).
    full_width: bool,
    /// The trigger's height. Defaults to the compact 24px that lines up with
    /// [`Select`](super::select::Select) in a settings row.
    height: Pixels,
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
            // This field doesn't opt into `emit_tab`/`emit_nav`, so these never fire.
            TextInputEvent::Tab
            | TextInputEvent::BackTab
            | TextInputEvent::Up
            | TextInputEvent::Down => {}
        })
        .detach();

        Self {
            id: id.into(),
            // A tab stop so Tab reaches the closed trigger (which tracks this handle),
            // and the shared ancestor of the open popover's search field — so one
            // `is_focused` check covers the combo in either state.
            focus_handle: cx.focus_handle().tab_stop(true),
            input,
            options: Vec::new(),
            current: None,
            selected: Vec::new(),
            multi: false,
            filtered: Vec::new(),
            cursor: 0,
            open: false,
            needs_focus: false,
            placeholder: "Select…".into(),
            chevron: None,
            check: None,
            leading: None,
            trailing: None,
            full_width: false,
            height: px(24.),
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

    /// Whether the combo holds keyboard focus — either the closed trigger, or the
    /// open popover's search field (a descendant of the shared focus handle). Lets
    /// an owner react to the combo being focused (e.g. scroll it into view).
    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
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

    /// Disclosure glyph for the trigger, as a factory re-invoked each render so it
    /// re-themes with the app. Falls back to a stacked unicode chevron when unset.
    pub fn set_chevron(
        &mut self,
        make: impl Fn(&App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.chevron = Some(Box::new(make));
        cx.notify();
    }

    /// Mark drawn on the selected row, as a per-render factory. Falls back to a
    /// unicode check when unset.
    pub fn set_check(
        &mut self,
        make: impl Fn(&App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.check = Some(Box::new(make));
        cx.notify();
    }

    /// A leading element drawn before each option's label — in the trigger for the
    /// current selection and on every popover row. The factory is keyed by the
    /// option's index (its position in the list passed to [`set_options`]) and
    /// re-invoked each render so it re-themes. Unset = label only.
    pub fn set_leading(
        &mut self,
        make: impl Fn(usize, &App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.leading = Some(Box::new(make));
        cx.notify();
    }

    /// A trailing element drawn after each option's label on the popover rows —
    /// a match count, a shortcut hint. Keyed by the option's index like
    /// [`set_leading`](Self::set_leading), and re-invoked each render so it
    /// re-themes. Rows only: the trigger has no room for it.
    pub fn set_trailing(
        &mut self,
        make: impl Fn(usize, &App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.trailing = Some(Box::new(make));
        cx.notify();
    }

    /// Whether the trigger stretches to fill its parent's width. Off by default
    /// (the trigger sizes to content); on for form fields that line up with
    /// full-width inputs above and below them.
    pub fn set_full_width(&mut self, full: bool, cx: &mut Context<Self>) {
        self.full_width = full;
        cx.notify();
    }

    /// The trigger's height, for a combo that has to line up with taller
    /// neighbours (a filter dropdown beside a search field). Defaults to the
    /// compact 24px of a settings row.
    pub fn set_trigger_height(&mut self, height: Pixels, cx: &mut Context<Self>) {
        self.height = height;
        cx.notify();
    }

    /// Switch between picking one option and picking a set.
    ///
    /// In multi-select a pick toggles instead of replacing and the popover stays
    /// open, so choosing three values is three clicks rather than three visits;
    /// the trigger summarises the set and a Clear appears beside the search
    /// field. Feed the set with [`set_options_selected`](Self::set_options_selected)
    /// and react to [`ComboBoxEvent::Toggle`] / [`ComboBoxEvent::Clear`].
    pub fn set_multi(&mut self, multi: bool, cx: &mut Context<Self>) {
        self.multi = multi;
        cx.notify();
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

    /// The multi-select twin of [`set_options`](Self::set_options): replace the
    /// option list and the whole selected set. Indices past the end of `options`
    /// are dropped, so an owner can hand over a shrinking list without first
    /// pruning its own set.
    pub fn set_options_selected(
        &mut self,
        options: Vec<SharedString>,
        selected: Vec<usize>,
        cx: &mut Context<Self>,
    ) {
        self.options = options;
        self.selected = selected
            .into_iter()
            .filter(|&ix| ix < self.options.len())
            .collect();
        self.refilter(cx);
        cx.notify();
    }

    /// Whether `option` carries a check, in either mode.
    fn is_selected(&self, option: usize) -> bool {
        if self.multi {
            self.selected.contains(&option)
        } else {
            self.current == Some(option)
        }
    }

    /// Drop the whole multi-select set (the popover's Clear). A no-op in
    /// single-select, which has no "nothing chosen" gesture.
    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        if !self.multi || self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        cx.emit(ComboBoxEvent::Clear);
        cx.notify();
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.needs_focus = true;
        // Each open starts from a clean query, and from no cursor history — so
        // `refilter` parks the cursor on the current selection rather than
        // holding wherever the last visit left it.
        self.input.update(cx, |input, cx| input.set_content("", cx));
        self.filtered.clear();
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
        // Which option the cursor sits on right now. A refresh that leaves the
        // list intact (a multi-select owner feeding back an updated selection
        // mid-visit) must not throw the keyboard cursor back to the top.
        let anchored = self.filtered.get(self.cursor).map(|f| f.option);
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
        // Hold the cursor where it was if that option is still listed; else park
        // it on the current selection when that survived the filter (so opening
        // with an empty query highlights what's already chosen); else the first
        // match. In multi-select the first selected option stands in for "the
        // current one".
        let anchor = if self.multi {
            self.selected.first().copied()
        } else {
            self.current
        };
        self.cursor = anchored
            .and_then(|opt| self.filtered.iter().position(|f| f.option == opt))
            .or_else(|| anchor.and_then(|cur| self.filtered.iter().position(|f| f.option == cur)))
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
        let Some(f) = self.filtered.get(cursor) else {
            return;
        };
        let option = f.option;
        let label = self.options[option].clone();
        // The pick lands on our own state straight away, so the trigger and the
        // checks reflect it without waiting for the owner to feed it back.
        if self.multi {
            // Toggle in place and stay open, query intact: choosing several
            // values is the whole point of the mode, and reopening between each
            // would make the third pick cost as much as the first.
            let selected = match self.selected.iter().position(|&ix| ix == option) {
                Some(at) => {
                    self.selected.remove(at);
                    false
                }
                None => {
                    self.selected.push(option);
                    true
                }
            };
            cx.emit(ComboBoxEvent::Toggle { label, selected });
        } else {
            self.current = Some(option);
            self.open = false;
            cx.emit(ComboBoxEvent::Select(label));
        }
        cx.notify();
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
        // Multi-select summarises the set as `First +2` rather than listing it:
        // the trigger is one line, and the first pick plus a count says more in
        // that line than two truncated labels would.
        let lead_option = if self.multi {
            self.selected.first().copied()
        } else {
            self.current
        };
        let has_selection = lead_option.is_some();
        let current_label = match lead_option.and_then(|ix| self.options.get(ix).cloned()) {
            None => self.placeholder.clone(),
            Some(first) if self.multi && self.selected.len() > 1 => {
                SharedString::from(format!("{first} +{}", self.selected.len() - 1))
            }
            Some(first) => first,
        };
        // Caller's disclosure glyph (lucide chevron), else a stacked unicode mark.
        let chevron = match self.chevron.as_ref() {
            Some(make) => make(cx),
            None => div()
                .flex()
                .flex_col()
                .items_center()
                .text_color(theme.accent)
                .text_size(theme.font_size_micro())
                .line_height(px(6.))
                .child("⌃")
                .child("⌄")
                .into_any_element(),
        };
        // The leading element (colour dot / glyph) of the option the trigger
        // names, if any.
        let trigger_leading =
            lead_option.and_then(|ix| self.leading.as_ref().map(|make| make(ix, cx)));
        let trigger = div()
            .id(self.id.clone())
            .role(Role::ComboBox)
            .aria_label(current_label.clone())
            .aria_expanded(open)
            .flex()
            .items_center()
            .gap_1p5()
            .h(self.height)
            .when(self.full_width, |t| t.w_full())
            .px_2()
            .rounded(theme.radius)
            .bg(theme.bg_input)
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border
            })
            .text_size(theme.font_size_sm())
            // The selected value reads in the normal foreground (not the accent) —
            // on a strongly-accented theme an accent-coloured value glows.
            .text_color(if has_selection {
                theme.text
            } else {
                theme.text_faint
            })
            .cursor_pointer()
            // Tab reaches the closed trigger via the shared focus handle (a tab
            // stop); GPUI fires the click on Enter/Space, so the focused combo opens
            // from the keyboard. Tracked only while closed — when open the popover
            // owns the handle and the search field holds focus.
            .when(!open, |this| this.track_focus(&self.focus_handle))
            .focus(|s| s.border_color(theme.accent))
            .when_some(trigger_leading, |t, el| t.child(el))
            .child(div().flex_1().min_w_0().child(current_label))
            .child(chevron)
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
        let font_sm = theme.font_size_sm();
        let view = cx.entity().downgrade();
        let cursor = self.cursor;

        // Per-row elements built up front (each borrows `cx`), then moved into the
        // rows below by index. Multi-select checks every selected row, not just
        // one, so the check is built per row rather than once and moved.
        let mut row_checks: Vec<Option<AnyElement>> = self
            .filtered
            .iter()
            .map(|f| {
                self.is_selected(f.option)
                    .then(|| match self.check.as_ref() {
                        Some(make) => make(cx),
                        None => div()
                            .flex_none()
                            .text_size(font_sm)
                            .text_color(accent)
                            .child("✓")
                            .into_any_element(),
                    })
            })
            .collect();
        let mut row_leadings: Vec<Option<AnyElement>> = self
            .filtered
            .iter()
            .map(|f| self.leading.as_ref().map(|make| make(f.option, cx)))
            .collect();
        let mut row_trailings: Vec<Option<AnyElement>> = self
            .filtered
            .iter()
            .map(|f| self.trailing.as_ref().map(|make| make(f.option, cx)))
            .collect();

        let rows: Vec<_> = self
            .filtered
            .iter()
            .enumerate()
            .map(|(row_ix, f)| {
                let is_cursor = row_ix == cursor;
                let is_current = self.is_selected(f.option);
                let view = view.clone();
                let check = row_checks[row_ix].take();
                let leading = row_leadings[row_ix].take();
                let trailing = row_trailings[row_ix].take();
                div()
                    .id(("combo-row", row_ix))
                    .role(Role::ListBoxOption)
                    .aria_label(self.options[f.option].clone())
                    .aria_selected(is_current)
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .px(px(10.))
                    .py(px(6.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(is_cursor, |d| d.bg(bg_selected))
                    .when(!is_cursor, |d| d.hover(move |s| s.bg(bg_hover)))
                    .when_some(leading, |d, el| d.child(el))
                    .child(div().flex_1().min_w_0().child(highlighted_label(
                        &self.options[f.option],
                        &f.positions,
                        text,
                        accent,
                    )))
                    .when_some(trailing, |d, el| d.child(el))
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
                .text_size(font_sm)
                .text_color(text_faint)
                .child("No matches")
                .into_any_element()
        } else {
            div()
                .id("combo-list")
                .role(Role::ListBox)
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(6.))
                .max_h(px(320.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };

        // Multi-select needs a way back to "no filter" that isn't unticking each
        // value in turn; single-select has no such state to return to.
        let clear = (self.multi && !self.selected.is_empty()).then(|| {
            div()
                .id("combo-clear")
                .flex_none()
                .px_1p5()
                .py(px(1.))
                .rounded(theme.radius_sm)
                .text_size(theme.font_size_xs())
                .text_color(text_faint)
                .cursor_pointer()
                .hover(move |s| s.text_color(text).bg(bg_hover))
                .child("Clear")
                .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx)))
        });

        let input_row = div()
            .flex()
            .items_center()
            .gap_2()
            .px(px(12.))
            .py(px(9.))
            .border_b_1()
            .border_color(theme.border)
            .text_size(font_sm)
            .child(div().flex_1().min_w_0().child(self.input.clone()))
            .children(clear);

        let panel = div()
            .id("combo-popover")
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(240.))
            .font_family(theme.font_family.clone())
            .text_size(font_sm)
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
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
