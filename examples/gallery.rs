// SPDX-License-Identifier: GPL-3.0-or-later

//! The component gallery - `flint`'s "storybook".
//!
//! Run with `cargo run -p flint --example gallery`. It installs a theme global
//! and renders every component in its key states. The header toggles One Dark ↔
//! GitHub Dark so theming is verifiable at a glance.

use gpui::{
    anchored, deferred, div, prelude::*, App, Bounds, Context, Entity, MouseButton, Pixels, Point,
    SharedString, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;
use flint::prelude::*;

/// Demo rows for the `Table` section: `(name, size, modified)`.
const ROWS: &[(&str, &str, &str)] = &[
    ("assets", "—", "2026-05-31 14:02"),
    ("src", "—", "2026-06-01 09:18"),
    ("Cargo.toml", "1.2 KB", "2026-06-02 11:44"),
    ("README.md", "4.8 KB", "2026-06-02 12:01"),
    ("build.log", "320 KB", "2026-06-03 08:55"),
    (".gitignore", "64 B", "2026-05-30 17:30"),
];

/// One row in the in-app-drag table demo. `parent` is set once the item is
/// dragged into a folder, which hides it from the top level.
#[derive(Clone)]
struct DragItem {
    name: SharedString,
    is_dir: bool,
    parent: Option<SharedString>,
}

struct Gallery {
    name_input: Entity<TextInput>,
    host_input: Entity<TextInput>,
    password_input: Entity<TextInput>,
    tab: usize,
    modal_open: bool,
    selected_row: Option<usize>,
    sort: Option<(usize, bool)>,
    toggle_on: bool,
    segment: usize,
    select: usize,
    select_open: bool,
    /// The right-clicked row + cursor position for the secondary-click table demo.
    row_menu: Option<(usize, Point<Pixels>)>,
    /// Backing list for the in-app drag-to-folder table demo.
    drag_items: Vec<DragItem>,
}

impl Gallery {
    fn new(cx: &mut App) -> Self {
        Self {
            name_input: cx.new(|cx| TextInput::new(cx).with_content("Production")),
            host_input: cx.new(|cx| TextInput::new(cx).with_placeholder("sftp.example.com")),
            password_input: cx.new(|cx| TextInput::new(cx).with_placeholder("password").obscured()),
            tab: 0,
            modal_open: false,
            selected_row: Some(2),
            sort: Some((0, true)),
            toggle_on: true,
            segment: 1,
            select: 0,
            select_open: false,
            row_menu: None,
            drag_items: vec![
                DragItem {
                    name: "Documents".into(),
                    is_dir: true,
                    parent: None,
                },
                DragItem {
                    name: "Pictures".into(),
                    is_dir: true,
                    parent: None,
                },
                DragItem {
                    name: "notes.txt".into(),
                    is_dir: false,
                    parent: None,
                },
                DragItem {
                    name: "todo.md".into(),
                    is_dir: false,
                    parent: None,
                },
                DragItem {
                    name: "photo.png".into(),
                    is_dir: false,
                    parent: None,
                },
            ],
        }
    }

    fn section(
        &self,
        title: impl Into<SharedString>,
        content: impl IntoElement,
        cx: &App,
    ) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_dim)
                    .child(title.into()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(content),
            )
    }

    fn buttons(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .gap_3()
            .items_center()
            .child(Button::new("primary", "Primary").variant(ButtonVariant::Primary))
            .child(Button::new("secondary", "Secondary").variant(ButtonVariant::Secondary))
            .child(Button::new("ghost", "Ghost").variant(ButtonVariant::Ghost))
            .child(Button::new("danger", "Danger").variant(ButtonVariant::Danger))
            .child(
                Button::new("disabled", "Disabled")
                    .variant(ButtonVariant::Primary)
                    .disabled(true),
            )
            .child(
                Button::new("small", "Small")
                    .variant(ButtonVariant::Secondary)
                    .size(ButtonSize::Sm),
            )
            .child(
                Button::new("with-icon", "Upload")
                    .variant(ButtonVariant::Ghost)
                    .icon("⬆"),
            )
    }

    fn icon_buttons(&self) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .items_center()
            .child(IconButton::new("ib-add", "＋").size(IconButtonSize::Sm))
            .child(IconButton::new("ib-refresh", "⟳").size(IconButtonSize::Md))
            .child(IconButton::new("ib-settings", "⚙").active(true))
            .child(IconButton::new("ib-close", "✕").disabled(true))
    }

    fn badges(&self) -> impl IntoElement {
        div()
            .flex()
            .gap_2()
            .items_center()
            .child(Badge::new("SFTP").variant(BadgeVariant::Special))
            .child(Badge::new("FTPS").variant(BadgeVariant::Success))
            .child(Badge::new("FTP").variant(BadgeVariant::Info))
            .child(Badge::new("Connected").variant(BadgeVariant::Success))
            .child(Badge::new("Error").variant(BadgeVariant::Danger))
            .child(Badge::new("Beta").variant(BadgeVariant::Neutral))
            .child(Badge::new("New").variant(BadgeVariant::Accent))
    }

    fn inputs(&self) -> impl IntoElement {
        div()
            .flex()
            .gap_3()
            .items_center()
            .child(div().w(gpui::px(200.)).child(self.name_input.clone()))
            .child(div().w(gpui::px(240.)).child(self.host_input.clone()))
            .child(div().w(gpui::px(160.)).child(self.password_input.clone()))
    }

    fn progress(&self, cx: &App) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_3()
            .w(gpui::px(280.))
            .child(ProgressBar::new("p-30", 0.3))
            .child(ProgressBar::new("p-70", 0.7))
            .child(ProgressBar::new("p-100", 1.0))
            .child(ProgressBar::new("p-indet", 0.0).indeterminate(true))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().text_faint)
                    .child("30% · 70% · 100% · indeterminate"),
            )
    }

    fn tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        Tabs::new("dock-tabs")
            .tab("Active", Some(3))
            .tab("Completed", Some(12))
            .tab("Failed", Some(1))
            .selected(self.tab)
            .on_select(move |ix, _window, cx| {
                view.update(cx, |this, cx| {
                    this.tab = ix;
                    cx.notify();
                });
            })
    }

    fn toggles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        div()
            .flex()
            .gap_3()
            .items_center()
            .child(
                Toggle::new("tg", self.toggle_on).on_change(move |on, _window, cx| {
                    let on = *on;
                    view.update(cx, |this, cx| {
                        this.toggle_on = on;
                        cx.notify();
                    });
                }),
            )
            .child(Toggle::new("tg-off", false))
            .child(Toggle::new("tg-disabled", true).disabled(true))
    }

    fn segmented(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        Segmented::new("seg")
            .segment("Compact")
            .segment("Comfortable")
            .segment("Spacious")
            .selected(self.segment)
            .on_select(move |ix, _window, cx| {
                view.update(cx, |this, cx| {
                    this.segment = ix;
                    cx.notify();
                });
            })
    }

    fn select(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let toggle_view = view.clone();
        // Constrain the width so the floating list has a sensible anchor.
        div().w(gpui::px(220.)).child(
            Select::new("sel")
                .option("One Dark")
                .option("GitHub Dark")
                .option("Ayu Dark")
                .selected(self.select)
                .open(self.select_open)
                .on_toggle(move |_window, cx| {
                    toggle_view.update(cx, |this, cx| {
                        this.select_open = !this.select_open;
                        cx.notify();
                    });
                })
                .on_select(move |ix, _window, cx| {
                    view.update(cx, |this, cx| {
                        this.select = ix;
                        this.select_open = false;
                        cx.notify();
                    });
                }),
        )
    }

    fn context_menu(&self) -> impl IntoElement {
        ContextMenu::new("demo-menu")
            .item(ContextMenuItem::new("download", "Download").shortcut("⌘D"))
            .item(ContextMenuItem::new("rename", "Rename").shortcut("F2"))
            .item(ContextMenuItem::new("copy-path", "Copy path"))
            .separator()
            .item(
                ContextMenuItem::new("delete", "Delete")
                    .shortcut("⌫")
                    .danger(),
            )
    }

    fn toasts(&self) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Toast::new("Connected to Production").variant(ToastVariant::Success))
            .child(Toast::new("Uploading 3 files…").variant(ToastVariant::Info))
            .child(Toast::new("Permission denied").variant(ToastVariant::Error))
    }

    fn tooltip_demo(&self) -> impl IntoElement {
        div()
            .id("tooltip-target")
            .px_3()
            .py_1p5()
            .rounded_md()
            .text_sm()
            .child("Hover me")
            .tooltip(Tooltip::text("This is a tooltip"))
    }

    fn table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let select_view = view.clone();
        let theme = cx.theme();
        let dir_color = theme.blue;
        let muted = theme.text_muted;
        // Demonstrate the generic `sort_carets` slot: the caller supplies its own
        // caret glyph (here a chevron pair) instead of the built-in triangles.
        let caret = theme.accent;

        div()
            .h(gpui::px(200.))
            .w_full()
            .panel(cx)
            .rounded(theme.radius)
            .overflow_hidden()
            .child(
                Table::<()>::new(
                    "files",
                    vec![
                        Column::new("Name").flex().sortable(),
                        Column::new("Size")
                            .width(gpui::px(90.))
                            .align_end()
                            .sortable(),
                        Column::new("Modified").width(gpui::px(150.)).sortable(),
                    ],
                )
                .row_count(ROWS.len())
                .selected(self.selected_row)
                .sort(self.sort)
                .sort_carets(
                    move || {
                        div()
                            .text_xs()
                            .text_color(caret)
                            .child("⌃")
                            .into_any_element()
                    },
                    move || {
                        div()
                            .text_xs()
                            .text_color(caret)
                            .child("⌄")
                            .into_any_element()
                    },
                )
                .on_select(move |ix, _event, _window, cx| {
                    select_view.update(cx, |this, cx| {
                        this.selected_row = Some(ix);
                        cx.notify();
                    });
                })
                .on_sort(move |col, _window, cx| {
                    view.update(cx, |this, cx| {
                        this.sort = match this.sort {
                            Some((c, asc)) if c == col => Some((c, !asc)),
                            _ => Some((col, true)),
                        };
                        cx.notify();
                    });
                })
                .render_row(move |ix, _window, _cx| {
                    let (name, size, modified) = ROWS[ix];
                    let is_dir = size == "—";
                    vec![
                        div()
                            .text_color(if is_dir { dir_color } else { muted })
                            .child(name)
                            .into_any_element(),
                        div().text_color(muted).child(size).into_any_element(),
                        div().text_color(muted).child(modified).into_any_element(),
                    ]
                }),
            )
    }

    /// A table whose **right-click** opens a `ContextMenu` anchored at the cursor,
    /// exercising `Table::on_secondary` (index + position, no domain types).
    fn secondary_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = cx.theme();
        let dir_color = theme.blue;
        let muted = theme.text_muted;

        let table = div()
            .h(gpui::px(180.))
            .w_full()
            .panel(cx)
            .rounded(theme.radius)
            .overflow_hidden()
            .child(
                Table::<()>::new(
                    "secondary-files",
                    vec![
                        Column::new("Name").flex(),
                        Column::new("Size").width(gpui::px(90.)).align_end(),
                    ],
                )
                .row_count(ROWS.len())
                .on_secondary({
                    let view = view.clone();
                    move |ix, pos, _window, cx| {
                        view.update(cx, |this, cx| {
                            this.row_menu = Some((ix, pos));
                            cx.notify();
                        });
                    }
                })
                .render_row(move |ix, _window, _cx| {
                    let (name, size, _) = ROWS[ix];
                    let is_dir = size == "—";
                    vec![
                        div()
                            .text_color(if is_dir { dir_color } else { muted })
                            .child(name)
                            .into_any_element(),
                        div().text_color(muted).child(size).into_any_element(),
                    ]
                }),
            );

        div()
            .relative()
            .child(table)
            .when_some(self.row_menu, |this, (ix, pos)| {
                let name = ROWS[ix].0;
                let menu = ContextMenu::new("secondary-ctx")
                    .item(ContextMenuItem::new(
                        "s-download",
                        format!("Download {name}"),
                    ))
                    .item(ContextMenuItem::new("s-rename", "Rename"))
                    .separator()
                    .item(ContextMenuItem::new("s-delete", "Delete").danger());
                let dismiss_view = view.clone();
                this.child(
                    div()
                        .absolute()
                        .inset_0()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            dismiss_view.update(cx, |this, cx| {
                                this.row_menu = None;
                                cx.notify();
                            });
                        })
                        .child(deferred(
                            anchored().position(pos).child(div().occlude().child(menu)),
                        )),
                )
            })
    }

    /// An **in-app drag** table: drag any row onto a folder row to move it in
    /// (the item then disappears from the top level). Exercises `Table`'s
    /// `on_row_drag` (payload), `drag_preview` (cursor chip) and
    /// `on_row_drop_item` (typed in-app drop) - no OS involvement.
    fn drag_table(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let theme = cx.theme();
        let dir_color = theme.blue;
        let muted = theme.text_muted;
        let chip_bg = theme.bg_elevated;
        let chip_border = theme.border;
        let chip_text = theme.text;
        let radius = theme.radius;

        // Top-level rows are the not-yet-moved items.
        let visible: Vec<DragItem> = self
            .drag_items
            .iter()
            .filter(|i| i.parent.is_none())
            .cloned()
            .collect();
        let rows_droppable = visible.clone();
        let moved = self
            .drag_items
            .iter()
            .filter(|i| i.parent.is_some())
            .count();

        let rows_render = visible.clone();
        let rows_drag = visible.clone();
        let rows_preview = visible.clone();
        let rows_drop = visible.clone();

        let table = Table::<Vec<SharedString>>::new(
            "drag-files",
            vec![
                Column::new("Name").flex(),
                Column::new("Kind").width(gpui::px(80.)),
            ],
        )
        .row_count(visible.len())
        .draggable_rows(|_| true)
        .on_row_drag(move |ix| rows_drag.get(ix).map(|i| vec![i.name.clone()]))
        .drag_preview(move |ix, _window, _cx| match rows_preview.get(ix) {
            Some(item) => div()
                .flex()
                .items_center()
                .px_2()
                .py_1()
                .rounded(radius)
                .bg(chip_bg)
                .border_1()
                .border_color(chip_border)
                .text_sm()
                .text_color(chip_text)
                .child(item.name.clone())
                .into_any_element(),
            None => div().into_any_element(),
        })
        .droppable_rows(move |ix| rows_droppable.get(ix).is_some_and(|i| i.is_dir))
        .on_row_drop_item(move |ix, names: &Vec<SharedString>, _window, cx| {
            let Some(folder) = rows_drop
                .get(ix)
                .filter(|i| i.is_dir)
                .map(|i| i.name.clone())
            else {
                return;
            };
            let names = names.clone();
            view.update(cx, |this, cx| {
                for item in this.drag_items.iter_mut() {
                    if item.name != folder && names.contains(&item.name) {
                        item.parent = Some(folder.clone());
                    }
                }
                cx.notify();
            });
        })
        .render_row(move |ix, _window, _cx| {
            let item = &rows_render[ix];
            vec![
                div()
                    .text_color(if item.is_dir { dir_color } else { muted })
                    .child(item.name.clone())
                    .into_any_element(),
                div()
                    .text_color(muted)
                    .child(if item.is_dir { "Folder" } else { "File" })
                    .into_any_element(),
            ]
        });

        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .h(gpui::px(200.))
                    .w_full()
                    .panel(cx)
                    .rounded(theme.radius)
                    .overflow_hidden()
                    .child(table),
            )
            .child(div().text_xs().text_color(muted).child(format!(
                "Drag a row onto a folder to move it in - {moved} moved."
            )))
    }

    fn modal(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let close_view = cx.entity();
        let save_view = cx.entity();
        Modal::new("demo-modal")
            .title("Edit connection")
            .on_close(move |_window, cx| {
                close_view.update(cx, |this, cx| {
                    this.modal_open = false;
                    cx.notify();
                });
            })
            .child(
                div().flex().flex_col().gap_3().child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().text_muted)
                        .child("A demo modal. Click the scrim or ✕ to close."),
                ),
            )
            .footer(
                div()
                    .flex()
                    .w_full()
                    .justify_end()
                    .gap_2()
                    .child(Button::new("modal-cancel", "Cancel").variant(ButtonVariant::Ghost))
                    .child(
                        Button::new("modal-save", "Save")
                            .variant(ButtonVariant::Primary)
                            .on_click(move |_, _, cx| {
                                save_view.update(cx, |this, cx| {
                                    this.modal_open = false;
                                    cx.notify();
                                });
                            }),
                    ),
            )
    }
}

impl Render for Gallery {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_name = cx.theme().name.clone();
        let open_modal = cx.listener(|this, _, _, cx| {
            this.modal_open = true;
            cx.notify();
        });

        let header = div()
            .id("theme-toggle")
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap_1()
            .p_8()
            .pb_0()
            .child(div().text_xl().child("flint gallery"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().text_muted)
                    .child(format!("Theme: {theme_name}  (click to toggle)")),
            )
            .on_click(cx.listener(|_, _, _, cx| {
                let next = match cx.theme().name.as_str() {
                    "One Dark" => Theme::github_dark(),
                    "GitHub Dark" => Theme::ayu_dark(),
                    "Ayu Dark" => Theme::ayu_light(),
                    _ => Theme::one_dark(),
                };
                cx.set_global(next);
                cx.notify();
            }));

        let buttons = self.buttons();
        let icon_buttons = self.icon_buttons();
        let badges = self.badges();
        let inputs = self.inputs();
        let progress = self.progress(cx);
        let tabs = self.tabs(cx);
        let toggles = self.toggles(cx);
        let segmented = self.segmented(cx);
        let select = self.select(cx);
        let context_menu = self.context_menu();
        let toasts = self.toasts();
        let tooltip = self.tooltip_demo();
        let table = self.table(cx);
        let secondary_table = self.secondary_table(cx);
        let drag_table = self.drag_table(cx);

        let body = div()
            .id("scroll")
            .flex_1()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_6()
            .p_8()
            .child(self.section("Buttons", buttons, cx))
            .child(self.section("Icon buttons", icon_buttons, cx))
            .child(self.section("Badges", badges, cx))
            .child(self.section("Text inputs", inputs, cx))
            .child(self.section("Progress", progress, cx))
            .child(self.section("Tabs", tabs, cx))
            .child(self.section("Toggle", toggles, cx))
            .child(self.section("Segmented", segmented, cx))
            .child(self.section("Select", select, cx))
            .child(self.section("Context menu", context_menu, cx))
            .child(self.section("Toasts", toasts, cx))
            .child(self.section("Tooltip", tooltip, cx))
            .child(
                self.section(
                    "Modal",
                    Button::new("open-modal", "Open modal")
                        .variant(ButtonVariant::Secondary)
                        .on_click(open_modal),
                    cx,
                ),
            )
            .child(self.section("Table", table, cx))
            .child(self.section("Table - right-click menu", secondary_table, cx))
            .child(self.section("Table - in-app drag to folder", drag_table, cx));

        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(cx.theme().bg_app)
            .text_color(cx.theme().text)
            .child(header)
            .child(body)
            .when(self.modal_open, |this| {
                let modal = self.modal(cx);
                this.child(modal)
            })
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.set_global(Theme::one_dark());
        TextInput::bind_keys(cx);

        let bounds = Bounds::centered(None, gpui::size(gpui::px(960.0), gpui::px(720.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Gallery::new(cx)),
        )
        .expect("failed to open gallery window");
        cx.activate(true);
    });
}
