//! `Switcher` — a project/connection switcher: a small trigger (a coloured dot,
//! a label, a disclosure chevron) that opens a searchable, **sectioned** popover
//! anchored beneath it. Modelled on Zed's project switcher.
//!
//! Generic and domain-free like the rest of Flint: the owner hands it
//! [`SwitcherSection`]s of [`SwitcherItem`]s and reacts to
//! [`SwitcherEvent::Activate`] with the `id` of the chosen row — the switcher
//! knows nothing about what a row *means*. Each row carries an optional leading
//! colour dot, a secondary detail line, a state badge (e.g. warm / cold), and a
//! checkmark for the active row.
//!
//! Stateful (held in an `Entity`) because it owns the embedded search field, the
//! fuzzy filter, keyboard navigation, and its own open flag — the owner only
//! [`open`](Switcher::open)/[`toggle`](Switcher::toggle)s it and feeds it
//! sections. Call [`Switcher::bind_keys`] once at startup for ↑/↓ navigation.

use gpui::{
    actions, canvas, div, point, prelude::*, px, AnyElement, App, Bounds, Context, ElementId,
    Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Hsla, KeyBinding, Pixels,
    SharedString, Window,
};

use crate::components::floating::floating;
use crate::components::fuzzy::{fuzzy_match, highlighted_label};
use crate::components::text_input::{TextInput, TextInputEvent};
use crate::theme::ActiveTheme;

actions!(flint_switcher, [SelectNext, SelectPrev]);

/// A glyph factory, re-invoked each render so the icon re-themes with the app.
/// Caller-supplied so Flint stays domain-free (RED hands in its lucide SVGs).
type IconFn = Box<dyn Fn(&App) -> AnyElement + 'static>;

/// A small pill drawn on the right of a row, e.g. a connection's warm/cold state.
#[derive(Clone, Debug)]
pub struct SwitcherBadge {
    pub label: SharedString,
    pub color: Hsla,
}

impl SwitcherBadge {
    pub fn new(label: impl Into<SharedString>, color: Hsla) -> Self {
        Self {
            label: label.into(),
            color,
        }
    }
}

/// One selectable row. `id` is opaque to the switcher and handed straight back on
/// activation, so the owner maps it to whatever it means.
#[derive(Clone, Debug)]
pub struct SwitcherItem {
    pub id: ElementId,
    pub label: SharedString,
    /// A faint second line under the label (e.g. a host, or "5m ago").
    pub detail: Option<SharedString>,
    /// Leading colour dot. `None` renders a flush label (e.g. action rows).
    pub dot: Option<Hsla>,
    /// Trailing state pill.
    pub badge: Option<SwitcherBadge>,
    /// Draws a checkmark — the active / current row.
    pub checked: bool,
}

impl SwitcherItem {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            dot: None,
            badge: None,
            checked: false,
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn dot(mut self, color: Hsla) -> Self {
        self.dot = Some(color);
        self
    }

    pub fn badge(mut self, badge: SwitcherBadge) -> Self {
        self.badge = Some(badge);
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// A titled group of rows. A `None` title renders an untitled group (e.g. the
/// trailing actions), separated from the previous one only by spacing.
#[derive(Clone, Debug)]
pub struct SwitcherSection {
    pub title: Option<SharedString>,
    pub items: Vec<SwitcherItem>,
}

impl SwitcherSection {
    pub fn new(title: impl Into<SharedString>, items: Vec<SwitcherItem>) -> Self {
        Self {
            title: Some(title.into()),
            items,
        }
    }

    pub fn untitled(items: Vec<SwitcherItem>) -> Self {
        Self { title: None, items }
    }
}

/// What the owner subscribes to via `cx.subscribe`.
#[derive(Clone, Debug)]
pub enum SwitcherEvent {
    /// The user chose the row with this `id` (Enter or click).
    Activate(ElementId),
    /// The popover was dismissed (Escape or an outside click).
    Dismiss,
}

/// A filtered row: where it lives in `sections`, plus the matched byte offsets in
/// its label for highlighting. Stored in display order (section, then item).
struct Filtered {
    section: usize,
    item: usize,
    positions: Vec<usize>,
}

pub struct Switcher {
    id: SharedString,
    focus_handle: FocusHandle,
    input: Entity<TextInput>,
    sections: Vec<SwitcherSection>,
    /// Pinned action rows, rendered below the scrollable list and always
    /// visible (exempt from the search filter) — e.g. "New…" / "Manage…".
    footer: Vec<SwitcherItem>,
    filtered: Vec<Filtered>,
    /// Selection index over the combined navigable space: the `filtered` list
    /// rows first, then the `footer` rows.
    selected: usize,
    open: bool,
    /// Focus the search field on the next paint (set when opening).
    needs_focus: bool,
    /// Trigger label and its leading dot (e.g. the active connection).
    trigger_label: SharedString,
    trigger_dot: Option<Hsla>,
    /// Trigger disclosure glyph; falls back to a unicode chevron when unset.
    chevron: Option<IconFn>,
}

impl Switcher {
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
            TextInputEvent::Submit => this.activate(this.selected, cx),
            TextInputEvent::Cancel => this.dismiss(cx),
        })
        .detach();

        Self {
            id: id.into(),
            focus_handle: cx.focus_handle(),
            input,
            sections: Vec::new(),
            footer: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            open: false,
            needs_focus: false,
            trigger_label: "Switch…".into(),
            trigger_dot: None,
            chevron: None,
        }
    }

    /// Call once at startup. ↑/↓ (and Ctrl-P/Ctrl-N) navigate the list, scoped to
    /// the `"Switcher"` key context. Enter/Escape ride the search field's own
    /// `Submit`/`Cancel` bindings.
    pub fn bind_keys(cx: &mut App) {
        let ctx = Some("Switcher");
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

    /// Set how the (always-visible) trigger reads: its label and leading dot.
    pub fn set_trigger(
        &mut self,
        label: impl Into<SharedString>,
        dot: Option<Hsla>,
        cx: &mut Context<Self>,
    ) {
        self.trigger_label = label.into();
        self.trigger_dot = dot;
        cx.notify();
    }

    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.input
            .update(cx, |input, cx| input.set_placeholder(text, cx));
    }

    /// Disclosure glyph for the trigger, as a factory re-invoked each render so it
    /// re-themes with the app. Falls back to a unicode chevron when unset.
    pub fn set_chevron(
        &mut self,
        make: impl Fn(&App) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) {
        self.chevron = Some(Box::new(make));
        cx.notify();
    }

    /// Replace the popover contents and re-filter against the current query.
    pub fn set_sections(&mut self, sections: Vec<SwitcherSection>, cx: &mut Context<Self>) {
        self.sections = sections;
        self.refilter(cx);
        cx.notify();
    }

    /// Set the pinned footer rows — always-visible actions shown beneath the
    /// scrollable list and unaffected by the search query (e.g. "New…").
    pub fn set_footer(&mut self, items: Vec<SwitcherItem>, cx: &mut Context<Self>) {
        self.footer = items;
        self.refilter(cx);
        cx.notify();
    }

    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = true;
        self.needs_focus = true;
        self.selected = 0;
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
        cx.emit(SwitcherEvent::Dismiss);
        cx.notify();
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).content();
        let mut filtered = Vec::new();
        for (si, section) in self.sections.iter().enumerate() {
            for (ii, item) in section.items.iter().enumerate() {
                if let Some((_score, positions)) = fuzzy_match(&query, &item.label) {
                    filtered.push(Filtered {
                        section: si,
                        item: ii,
                        positions,
                    });
                }
            }
        }
        self.filtered = filtered;
        self.selected = 0;
    }

    /// Count of navigable rows: filtered list rows plus the pinned footer.
    fn nav_len(&self) -> usize {
        self.filtered.len() + self.footer.len()
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.nav_len();
        if len != 0 {
            self.selected = (self.selected + 1) % len;
            cx.notify();
        }
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        let len = self.nav_len();
        if len != 0 {
            self.selected = (self.selected + len - 1) % len;
            cx.notify();
        }
    }

    fn activate(&mut self, flat_ix: usize, cx: &mut Context<Self>) {
        // Indices past the filtered list address the pinned footer rows.
        let id = if let Some(f) = self.filtered.get(flat_ix) {
            Some(self.sections[f.section].items[f.item].id.clone())
        } else {
            self.footer
                .get(flat_ix - self.filtered.len())
                .map(|item| item.id.clone())
        };
        if let Some(id) = id {
            self.open = false;
            cx.emit(SwitcherEvent::Activate(id));
            cx.notify();
        }
    }
}

impl Focusable for Switcher {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl EventEmitter<SwitcherEvent> for Switcher {}

impl Render for Switcher {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.needs_focus {
            self.needs_focus = false;
            let handle = self.input.read(cx).focus_handle(cx);
            window.focus(&handle, cx);
        }

        let theme = cx.theme().clone();
        let open = self.open;

        // Anchor the popover to the trigger's *measured* window bounds (same
        // canvas trick as `Select`), so it drops from the trigger's bottom-left.
        let bounds_state = window.use_keyed_state(
            SharedString::from(format!("{}__sw_bounds", self.id)),
            cx,
            |_, _| None::<Bounds<Pixels>>,
        );
        let trigger_bounds = *bounds_state.read(cx);
        let measure = bounds_state.clone();

        // ----- trigger -----
        let dot = self.trigger_dot.map(|c| {
            div()
                .size(px(8.))
                .rounded_full()
                .flex_none()
                .bg(c)
                .into_any_element()
        });
        // Caller's disclosure glyph (lucide chevron), else a unicode mark. Wrapped
        // in a centred flex box so it sits on the trigger's vertical midline.
        let chevron = match self.chevron.as_ref() {
            Some(make) => make(cx),
            None => div()
                .text_size(theme.font_size_xs())
                .text_color(theme.text_dim)
                .child("⌄")
                .into_any_element(),
        };
        let trigger = div()
            .id(self.id.clone())
            // Swallow mouse events so the trigger doesn't double as its host's
            // window-drag region — without this a double-tap on the trigger also
            // hits a titlebar drag area behind it and zooms the window.
            .occlude()
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(24.))
            .px_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(if open {
                theme.border_strong
            } else {
                theme.border_soft
            })
            .when(open, |s| s.bg(theme.bg_active))
            .text_size(theme.font_size)
            .text_color(theme.text)
            .cursor_pointer()
            .when_some(dot, |this, dot| this.child(dot))
            .child(div().child(self.trigger_label.clone()))
            .child(div().flex().items_center().child(chevron))
            .child(
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
        let radius_sm = theme.radius_sm;
        let font_base = theme.font_size;
        let font_sm = theme.font_size_sm();
        let micro = theme.font_size_micro();
        let view = cx.entity().downgrade();
        let selected = self.selected;

        // Renders one selectable row. Shared by the scrollable list and the
        // pinned footer so both look and behave identically; `flat_ix` is the
        // row's index in the combined navigable space (see `activate`).
        let make_row = move |flat_ix: usize, item: &SwitcherItem, positions: &[usize]| {
            let is_selected = flat_ix == selected;
            let view = view.clone();

            let dot = match item.dot {
                Some(c) => div().size(px(8.)).rounded_full().flex_none().bg(c),
                None => div().size(px(8.)).flex_none(),
            };
            let label_block = div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(highlighted_label(&item.label, positions, text, accent))
                .when_some(item.detail.clone(), |this, detail| {
                    this.child(
                        div()
                            .text_size(micro)
                            .text_color(text_faint)
                            .truncate()
                            .child(detail),
                    )
                });
            let badge = item.badge.clone().map(|b| {
                div()
                    .flex_none()
                    .px_1p5()
                    .py(px(1.))
                    .rounded(radius_sm)
                    .bg(b.color.opacity(0.15))
                    .text_size(micro)
                    .text_color(b.color)
                    .child(b.label)
            });
            let check = item.checked.then(|| {
                div()
                    .flex_none()
                    .text_size(font_sm)
                    .text_color(accent)
                    .child("✓")
            });

            div()
                .id(("switcher-row", flat_ix))
                .flex()
                .items_center()
                .gap_2p5()
                .px(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .cursor_pointer()
                .when(is_selected, |d| d.bg(bg_selected))
                .when(!is_selected, |d| d.hover(move |s| s.bg(bg_hover)))
                .child(dot)
                .child(label_block)
                .when_some(badge, |this, badge| this.child(badge))
                .when_some(check, |this, check| this.child(check))
                .on_click(move |_, _, cx| {
                    view.update(cx, |this, cx| this.activate(flat_ix, cx)).ok();
                })
                .into_any_element()
        };

        let mut rows: Vec<AnyElement> = Vec::new();
        let mut last_section: Option<usize> = None;
        for (flat_ix, f) in self.filtered.iter().enumerate() {
            if last_section != Some(f.section) {
                last_section = Some(f.section);
                if let Some(title) = &self.sections[f.section].title {
                    rows.push(
                        div()
                            .px(px(10.))
                            .pt(px(if flat_ix == 0 { 2. } else { 8. }))
                            .pb(px(3.))
                            .text_size(micro)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(text_faint)
                            .child(title.to_uppercase())
                            .into_any_element(),
                    );
                }
            }
            let item = &self.sections[f.section].items[f.item];
            rows.push(make_row(flat_ix, item, &f.positions));
        }

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
                .id("switcher-list")
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(6.))
                .max_h(px(360.))
                .overflow_y_scroll()
                .children(rows)
                .into_any_element()
        };

        // Pinned footer: the actions stay put while the list above scrolls.
        let footer = (!self.footer.is_empty()).then(|| {
            let base = self.filtered.len();
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .p(px(6.))
                .border_t_1()
                .border_color(theme.border_soft)
                .children(
                    self.footer
                        .iter()
                        .enumerate()
                        .map(|(i, item)| make_row(base + i, item, &[])),
                )
        });

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
            .id("switcher-popover")
            .occlude()
            .flex()
            .flex_col()
            .w(px(320.))
            .font_family(theme.font_family.clone())
            .text_size(font_base)
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border_strong)
            .rounded(px(10.))
            .shadow_lg()
            .overflow_hidden()
            .key_context("Switcher")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.dismiss(cx)))
            .child(input_row)
            .child(body)
            .when_some(footer, |this, footer| this.child(footer));

        div()
            .child(trigger)
            .when(open, |this| match trigger_bounds {
                Some(b) => this.child(
                    floating(panel)
                        .at(b.bottom_left())
                        .offset(point(px(0.), px(6.))),
                ),
                None => this,
            })
    }
}
