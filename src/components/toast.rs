// SPDX-License-Identifier: GPL-3.0-or-later

//! `Toast` - a single notification card. Stacking, positioning and auto-dismiss
//! are the caller's concern; this is just the visual. A toast has a `title` and
//! an optional `detail` body (plain text, or a caller-supplied element such as a
//! selectable label). Optional affordances, all domain-free: a leading `icon`
//! (falls back to a variant-coloured dot), a trailing `actions` slot (falls back
//! to a close `✕` button wired to `on_close`), an expand/collapse toggle for a
//! long body, and a thin progress bar (used for long-running operations like an
//! export). The caller owns the lifecycle, IDs, timers and expand state.

use gpui::{
    div, prelude::*, px, relative, AnyElement, App, ElementId, FontWeight, Hsla, Pixels, Role,
    SharedString, Window,
};

use crate::theme::ActiveTheme;

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;
type ToggleHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ToastVariant {
    #[default]
    Info,
    Success,
    Warning,
    Error,
}

#[derive(IntoElement)]
pub struct Toast {
    id: Option<ElementId>,
    title: SharedString,
    variant: ToastVariant,
    detail: Option<SharedString>,
    detail_element: Option<AnyElement>,
    icon: Option<AnyElement>,
    actions: Option<AnyElement>,
    on_close: Option<CloseHandler>,
    on_toggle: Option<ToggleHandler>,
    expandable: bool,
    expanded: bool,
    width: Option<Pixels>,
    progress: Option<f32>,
}

impl Toast {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            title: title.into(),
            variant: ToastVariant::default(),
            detail: None,
            detail_element: None,
            icon: None,
            actions: None,
            on_close: None,
            on_toggle: None,
            expandable: false,
            expanded: false,
            width: None,
            progress: None,
        }
    }

    /// A stable id for this toast. Required for the toast to be reported to
    /// assistive technology as a live region: without it the card carries no
    /// accessibility node (and stacking several un-id'd toasts would otherwise
    /// collide on a global id). Pass the caller's own notification id.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub fn variant(mut self, variant: ToastVariant) -> Self {
        self.variant = variant;
        self
    }

    /// A secondary line (or paragraph) of plain text under the title. Ignored if
    /// [`detail_element`](Self::detail_element) is also set.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// A caller-supplied body element under the title (e.g. a selectable label).
    /// Takes precedence over [`detail`](Self::detail).
    pub fn detail_element(mut self, element: impl IntoElement) -> Self {
        self.detail_element = Some(element.into_any_element());
        self
    }

    /// A leading icon (already sized and tinted by the caller). Without one the
    /// toast shows a variant-coloured dot.
    pub fn icon(mut self, icon: impl IntoElement) -> Self {
        self.icon = Some(icon.into_any_element());
        self
    }

    /// Trailing action controls (e.g. copy + close buttons). Takes precedence
    /// over the built-in close button from [`on_close`](Self::on_close).
    pub fn actions(mut self, actions: impl IntoElement) -> Self {
        self.actions = Some(actions.into_any_element());
        self
    }

    /// Render a trailing `✕` button wired to `handler` (dismiss / cancel). Used
    /// only when no [`actions`](Self::actions) slot is provided.
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }

    /// Mark the body as long enough to clamp: collapsed it shows a few lines with
    /// a "Show more" toggle; expanded it shows the full body (scrolling if very
    /// tall). Drive the state with [`expanded`](Self::expanded) +
    /// [`on_toggle`](Self::on_toggle).
    pub fn expandable(mut self, expandable: bool) -> Self {
        self.expandable = expandable;
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = expanded;
        self
    }

    /// Handler for the expand/collapse toggle (flip the caller's `expanded`).
    pub fn on_toggle(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(handler));
        self
    }

    /// Fix the card width (the stack looks tidiest uniform). Without one the card
    /// sizes to its content down to a sensible minimum.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = Some(width);
        self
    }

    /// Render a thin progress bar under the body. `fraction` is clamped to
    /// `0.0..=1.0`.
    pub fn progress(mut self, fraction: f32) -> Self {
        self.progress = Some(fraction.clamp(0.0, 1.0));
        self
    }
}

fn tone(variant: ToastVariant, theme: &crate::Theme) -> Hsla {
    match variant {
        ToastVariant::Info => theme.accent,
        ToastVariant::Success => theme.green,
        ToastVariant::Warning => theme.yellow,
        ToastVariant::Error => theme.red,
    }
}

impl RenderOnce for Toast {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let tone = tone(self.variant, theme);
        // Errors/warnings persist and demand attention → assertive `Alert`.
        // Info/success are advisory → polite `Status`. (The pinned GPUI rev has
        // no explicit live-region setter; these roles carry implicit live
        // semantics on the platforms that support it.)
        let a11y_role = match self.variant {
            ToastVariant::Warning | ToastVariant::Error => Role::Alert,
            ToastVariant::Info | ToastVariant::Success => Role::Status,
        };
        // The body is whatever the caller gave; plain `detail` is the fallback.
        let has_body = self.detail_element.is_some() || self.detail.is_some();
        // The screen-reader label is the title plus the plain detail (a custom
        // detail element carries its own a11y).
        let a11y_message: SharedString = match (&self.detail, self.detail_element.is_some()) {
            (Some(detail), false) => format!("{}. {}", self.title, detail).into(),
            _ => self.title.clone(),
        };
        // A toast is reported to assistive technology only when the caller gave
        // it an id (so stacked toasts can't collide on a global id). The id is
        // also needed for the stateful node `.role()` lives on; without one we
        // fall back to a constant id and skip the role, leaving it out of the
        // a11y tree entirely.
        let report = self.id.is_some();
        let card_id: ElementId = self.id.clone().unwrap_or_else(|| "toast".into());

        // Leading icon, or a variant-coloured dot. Nudge it down so it aligns
        // with the title's first line when the card is top-aligned (has a body).
        let leading = match self.icon {
            Some(icon) => div()
                .flex_shrink_0()
                .when(has_body, |d| d.mt(px(1.)))
                .child(icon),
            None => div()
                .flex_shrink_0()
                .when(has_body, |d| d.mt(px(4.)))
                .child(div().size(px(8.)).rounded_full().bg(tone)),
        };

        // The body, clamped when collapsed and scrollable when expanded so a
        // long message never grows the card without bound.
        let body = has_body.then(|| {
            let inner: AnyElement = match self.detail_element {
                Some(element) => element,
                None => div()
                    .child(self.detail.unwrap_or_default())
                    .into_any_element(),
            };
            div()
                .id("toast-detail")
                .text_size(theme.font_size_xs())
                .text_color(theme.text_muted)
                .when(self.expandable && !self.expanded, |d| {
                    d.max_h(theme.scale(50.)).overflow_hidden()
                })
                .when(self.expandable && self.expanded, |d| {
                    d.max_h(theme.scale(220.)).overflow_y_scroll()
                })
                .child(inner)
        });

        // The "Show more"/"Show less" toggle, when the caller marked the body
        // long and wired a handler.
        let toggle = (self.expandable && self.on_toggle.is_some()).then(|| {
            let handler = self.on_toggle.unwrap();
            let label = if self.expanded {
                "Show less"
            } else {
                "Show more"
            };
            div()
                .id("toast-toggle")
                .text_size(theme.font_size_xs())
                .text_color(theme.accent)
                .cursor_pointer()
                .hover(|s| s.underline())
                .child(label)
                .on_click(move |_, window, cx| handler(window, cx))
        });

        let content = div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(theme.font_size_sm())
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(self.title),
            )
            .children(body)
            .children(toggle);

        // Trailing controls: the caller's `actions` slot, else the built-in
        // close button.
        let trailing: Option<AnyElement> = match (self.actions, self.on_close) {
            (Some(actions), _) => Some(actions),
            (None, Some(handler)) => Some(
                div()
                    .id("toast-close")
                    .role(Role::Button)
                    .aria_label("Dismiss")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(20.))
                    .flex_shrink_0()
                    .rounded(theme.radius_sm)
                    .text_color(theme.text_faint)
                    .cursor_pointer()
                    .hover(|s| s.bg(theme.bg_hover).text_color(theme.text))
                    .tab_index(0)
                    .focus(|s| s.bg(theme.bg_hover).text_color(theme.text))
                    .child("✕")
                    .on_click(move |_, window, cx| handler(window, cx))
                    .into_any_element(),
            ),
            (None, None) => None,
        };

        let row = div()
            .flex()
            .when(has_body, |r| r.items_start())
            .when(!has_body, |r| r.items_center())
            .gap_2p5()
            .child(leading)
            .child(content)
            .children(trailing);

        div()
            .id(card_id)
            .when(report, |card| card.role(a11y_role).aria_label(a11y_message))
            .flex()
            .flex_col()
            .gap_2()
            .map(|card| match self.width {
                Some(width) => card.w(width),
                None => card.min_w(px(220.)),
            })
            .px_3()
            .py_2p5()
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .rounded(px(8.))
            .shadow_lg()
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size)
            .text_color(theme.text)
            .child(row)
            .when_some(self.progress, |card, fraction| {
                card.child(
                    div()
                        .h(px(3.))
                        .w_full()
                        .rounded(px(3.))
                        .bg(theme.bg_input)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(fraction))
                                .rounded(px(3.))
                                .bg(tone),
                        ),
                )
            })
    }
}
