// SPDX-License-Identifier: GPL-3.0-or-later

//! `Modal` - a centered dialog over a dimming scrim. Body and footer are
//! arbitrary `impl IntoElement`. Place it last in a positioned container (or
//! behind a [`gpui::deferred`] layer) so it paints above the rest of the UI.

use gpui::{
    actions, div, prelude::*, AnyElement, App, ElementId, FocusHandle, KeyBinding, Role,
    SharedString, Window,
};

use crate::scrim::ScrimDismiss;
use crate::theme::ActiveTheme;

// Tab navigation within a focused modal. Scoped to the `Modal` key context (set
// on the scrim when the modal is given a focus handle), so it cycles a modal's
// own controls without touching tab behaviour elsewhere. A deeper context — a
// focused `TextInput` — keeps its own Tab handling and wins.
actions!(modal, [FocusNext, FocusPrev]);

type CloseHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;
type ConfirmHandler = Box<dyn Fn(&mut Window, &mut App) + 'static>;

#[derive(IntoElement)]
pub struct Modal {
    id: ElementId,
    title: Option<SharedString>,
    width: gpui::Pixels,
    body: Vec<AnyElement>,
    footer: Option<AnyElement>,
    on_close: Option<CloseHandler>,
    on_confirm: Option<ConfirmHandler>,
    focus_handle: Option<FocusHandle>,
}

impl Modal {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            title: None,
            width: gpui::px(540.),
            body: Vec::new(),
            footer: None,
            on_close: None,
            on_confirm: None,
            focus_handle: None,
        }
    }

    /// Install the modal's `Tab`/`Shift-Tab` focus-cycling bindings (scoped to the
    /// `Modal` context). Call once at startup, like the other component keymaps.
    pub fn bind_keys(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("tab", FocusNext, Some("Modal")),
            KeyBinding::new("shift-tab", FocusPrev, Some("Modal")),
        ]);
    }

    /// Without a title the header is omitted.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn width(mut self, width: gpui::Pixels) -> Self {
        self.width = width;
        self
    }

    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }

    /// Invoked by the × button, a scrim click, and (with a [`focus_handle`]) the
    /// `Esc` key.
    ///
    /// [`focus_handle`]: Self::focus_handle
    pub fn on_close(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_close = Some(Box::new(handler));
        self
    }

    /// The primary action, invoked by `Enter` when the modal holds keyboard focus
    /// (requires a [`focus_handle`]). Pair it with a focused primary button.
    ///
    /// [`focus_handle`]: Self::focus_handle
    pub fn on_confirm(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_confirm = Some(Box::new(handler));
        self
    }

    /// Give the modal keyboard focus via a caller-owned handle, so `Esc` closes
    /// and `Enter` confirms. The caller focuses the handle when it opens the modal
    /// (the element is stateless, so it can't grab focus itself). Modals whose
    /// body owns the focus (a form field) should leave this unset and focus that
    /// field instead.
    pub fn focus_handle(mut self, handle: FocusHandle) -> Self {
        self.focus_handle = Some(handle);
        self
    }
}

impl ParentElement for Modal {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.body.extend(elements);
    }
}

impl RenderOnce for Modal {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let on_close = self.on_close.map(std::rc::Rc::new);
        let on_confirm = self.on_confirm.map(std::rc::Rc::new);
        let focus_handle = self.focus_handle;
        // The dialog's accessible name, reported on the panel's `Dialog` node.
        let a11y_title = self.title.clone();

        let header = self.title.map(|title| {
            let close = on_close.clone();
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .px_4()
                .py_3p5()
                .border_b_1()
                .border_color(theme.border)
                .child(
                    div()
                        .flex_1()
                        .text_size(theme.font_size)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .id("modal-close")
                        .role(Role::Button)
                        .aria_label("Close")
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(gpui::px(24.))
                        .rounded(theme.radius_sm)
                        .text_color(theme.text_faint)
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.bg_hover).text_color(theme.text))
                        .child("✕")
                        .when_some(close, |this, close| {
                            // Focusable so Tab reaches it, but `tab_index(1)` keeps
                            // it last in the ring — the close ✕ must never be a
                            // modal's initial focus (the first field/button is).
                            this.tab_index(1)
                                .focus(|s| s.bg(theme.bg_hover).text_color(theme.text))
                                .on_click(move |_, window, cx| close(window, cx))
                        }),
                )
        });

        let footer = self.footer.map(|footer| {
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .px_4()
                .py_3()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.bg_bar)
                // Match the panel's rounding so the bar's own fill doesn't paint
                // sharp corners into the rounded bottom (overflow_hidden clips
                // content, not a child's background corners).
                .rounded_b(gpui::px(9.))
                .child(footer)
        });

        let panel = div()
            .id("modal-panel")
            // A dialog landmark with the title as its accessible name, so screen
            // readers announce the modal on open and offer it to rotor navigation.
            .role(Role::Dialog)
            .when_some(a11y_title, |this, title| this.aria_label(title))
            .occlude()
            .flex()
            .flex_col()
            .w(self.width)
            .max_h(gpui::relative(0.88))
            .font_family(theme.font_family.clone())
            .text_size(theme.font_size)
            .bg(theme.bg_elevated)
            .border_1()
            .border_color(theme.border)
            .rounded(gpui::px(9.))
            .shadow_lg()
            .overflow_hidden()
            .when_some(header, |this, header| this.child(header))
            .child(
                div()
                    .id("modal-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .p_4()
                    .children(self.body),
            )
            .when_some(footer, |this, footer| this.child(footer));

        // Key handling rides on the scrim (an ancestor of every focusable child),
        // so it fires whether the modal root or a child button holds focus. `Esc`
        // closes, `Enter` confirms — both no-op without their handler.
        let key_close = on_close.clone();
        let key_confirm = on_confirm.clone();

        div()
            .id(self.id)
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.55))
            // Block all mouse interaction with the UI behind the scrim, so a
            // click that dismisses the modal can't also fire an element under it.
            .occlude()
            .when_some(focus_handle, |this, handle| {
                this.track_focus(&handle)
                    .key_context("Modal")
                    // Tab cycles the modal's own controls (the focus trap keeps it
                    // from escaping to the backdrop is the caller's to install).
                    .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                    .on_action(|_: &FocusPrev, window, cx| window.focus_prev(cx))
                    .on_key_down(
                        move |event, window, cx| match event.keystroke.key.as_str() {
                            "escape" => {
                                if let Some(close) = key_close.as_ref() {
                                    close(window, cx);
                                }
                            }
                            "enter" => {
                                if let Some(confirm) = key_confirm.as_ref() {
                                    confirm(window, cx);
                                }
                            }
                            _ => {}
                        },
                    )
            })
            .when_some(on_close, |this, close| {
                // Dismiss on a backdrop click — but not the click that ends a
                // window drag begun on the scrim (grabbing the title bar behind
                // the modal must move the window, not close it). See `ScrimDismiss`.
                this.on_scrim_dismiss(move |window, cx| close(window, cx))
            })
            .child(panel)
    }
}
