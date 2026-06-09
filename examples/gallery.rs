// SPDX-License-Identifier: GPL-3.0-or-later

//! The component gallery - `flint`'s "storybook".
//!
//! Run with `cargo run -p flint --example gallery`. It installs a theme global
//! and renders every component in its key states. The header toggles One Dark ↔
//! GitHub Dark so theming is verifiable at a glance.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flint::prelude::*;
use gpui::{
    anchored, deferred, div, prelude::*, px, App, Axis, Bounds, Context, Entity, MouseButton,
    Pixels, Point, SharedString, UniformListScrollHandle, Window, WindowBounds, WindowOptions,
};
use gpui_platform::application;

// Spike modules live in `gallery/` so Cargo doesn't treat them as separate
// example targets; `#[path]` is needed because this example root resolves
// `mod` siblings in `examples/`, not `examples/gallery/`.
#[path = "gallery/editor.rs"]
mod editor;
#[path = "gallery/sql.rs"]
mod sql;
#[path = "gallery/streaming.rs"]
mod streaming;
use editor::{CodeEditor, CodeEditorEvent};
use streaming::{
    CellColors, RowSource, SlowShared, SqliteSource, SyntheticSource, WindowBuffer, SQLITE_ROWS,
};

const SAMPLE_SQL: &str = "-- M0 spike B: a multiline SQL editor surface\nSELECT u.id, u.email, count(o.id) AS orders\nFROM users u\nLEFT JOIN orders o ON o.user_id = u.id\nWHERE u.active = true AND u.score >= 42.0\nGROUP BY u.id, u.email\nHAVING count(o.id) > 0\nORDER BY orders DESC\nLIMIT 100;";

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

    // --- M0 spike A: streaming grid ---
    /// 0 = synthetic (10M), 1 = SQLite (2M).
    stream_source_ix: usize,
    stream_buffer: Rc<RefCell<WindowBuffer>>,
    stream_scroll: UniformListScrollHandle,
    synthetic: Rc<SyntheticSource>,
    /// Built lazily once the background-generated DB is ready.
    sqlite: Option<Rc<SqliteSource>>,
    db_ready: Arc<AtomicBool>,
    /// The long-query cancel demo's shared state.
    slow: Arc<Mutex<SlowShared>>,

    // --- M0 spike B: multiline SQL editor ---
    sql_editor: Entity<CodeEditor>,
    editor_readonly: bool,
    /// A one-line summary of the last query "run" (⌘↵ or the Run button).
    last_run: Option<SharedString>,

    // --- SplitPane demo: caller-owned sizes + in-flight drag anchors ---
    split_h_size: Pixels,
    split_h_drag: Option<DragAnchor>,
    split_v_size: Pixels,
    split_v_drag: Option<DragAnchor>,

    // --- Tree demo: caller-owned expansion (by node path) + selection ---
    tree_expanded: HashSet<String>,
    tree_selected: Option<usize>,
}

impl Gallery {
    fn new(cx: &mut Context<Self>, db_ready: Arc<AtomicBool>) -> Self {
        let sql_editor = cx.new(|cx| {
            CodeEditor::new(cx)
                .with_content(SAMPLE_SQL)
                .highlighter(sql::tokenize)
        });
        // ⌘↵ in the editor emits Run; mirror it into the "last run" readout.
        cx.subscribe(&sql_editor, |this, editor, _event: &CodeEditorEvent, cx| {
            let sql = editor.read(cx).content();
            this.last_run = Some(summarize_sql(&sql));
            cx.notify();
        })
        .detach();
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

            stream_source_ix: 0,
            stream_buffer: Rc::new(RefCell::new(WindowBuffer::default())),
            stream_scroll: UniformListScrollHandle::new(),
            synthetic: Rc::new(SyntheticSource::default()),
            sqlite: None,
            db_ready,
            slow: Arc::new(Mutex::new(SlowShared::default())),

            sql_editor,
            editor_readonly: false,
            last_run: None,

            split_h_size: px(200.),
            split_h_drag: None,
            split_v_size: px(120.),
            split_v_drag: None,

            tree_expanded: ["src", "src/components"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            tree_selected: None,
        }
    }

    /// Nested resizable split: a sidebar (horizontal) wrapping a stacked
    /// editor/results pair (vertical). Drag either divider to resize.
    fn split_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (bg_left, bg_top, bg_bottom) = (theme.bg_panel_2, theme.bg_app, theme.bg_panel);
        let (muted, border, radius) = (theme.text_muted, theme.border, theme.radius);
        let view = cx.entity();

        let pane = move |label: &'static str, bg| {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .bg(bg)
                .text_color(muted)
                .text_sm()
                .child(label)
        };

        let inner = SplitPane::new("gallery-split-v", Axis::Vertical)
            .size(self.split_v_size)
            .drag(self.split_v_drag)
            .min_first(px(48.))
            .on_drag_start({
                let v = view.clone();
                move |a, _, cx| {
                    v.update(cx, |t, cx| {
                        t.split_v_drag = Some(a);
                        cx.notify();
                    })
                }
            })
            .on_resize({
                let v = view.clone();
                move |s, _, cx| {
                    v.update(cx, |t, cx| {
                        t.split_v_size = s;
                        cx.notify();
                    })
                }
            })
            .on_drag_end({
                let v = view.clone();
                move |_, cx| {
                    v.update(cx, |t, cx| {
                        t.split_v_drag = None;
                        cx.notify();
                    })
                }
            })
            .first(pane("Editor (top)", bg_top))
            .second(pane("Results (bottom)", bg_bottom));

        let outer = SplitPane::new("gallery-split-h", Axis::Horizontal)
            .size(self.split_h_size)
            .drag(self.split_h_drag)
            .min_first(px(120.))
            .max_first(px(420.))
            .on_drag_start({
                let v = view.clone();
                move |a, _, cx| {
                    v.update(cx, |t, cx| {
                        t.split_h_drag = Some(a);
                        cx.notify();
                    })
                }
            })
            .on_resize({
                let v = view.clone();
                move |s, _, cx| {
                    v.update(cx, |t, cx| {
                        t.split_h_size = s;
                        cx.notify();
                    })
                }
            })
            .on_drag_end({
                let v = view.clone();
                move |_, cx| {
                    v.update(cx, |t, cx| {
                        t.split_h_drag = None;
                        cx.notify();
                    })
                }
            })
            .first(pane("Schema (left)", bg_left))
            .second(inner);

        div()
            .w_full()
            .h(px(280.))
            .rounded(radius)
            .overflow_hidden()
            .border_1()
            .border_color(border)
            .child(outer)
    }

    /// A virtualized disclosure tree. The example owns expansion (a set of node
    /// paths) and selection; the component draws indent + chevron and reports
    /// clicks by index, exactly as RED's schema explorer will drive it.
    fn tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (dir_color, muted, radius, border) =
            (theme.blue, theme.text_muted, theme.radius, theme.border);
        let view = cx.entity();

        let mut flat = Vec::new();
        flatten_demo(DEMO_TREE, 0, "", &self.tree_expanded, &mut flat);
        let items: Vec<TreeItem> = flat.iter().map(|r| r.item).collect();
        let rows = Rc::new(flat);

        let rows_for_render = rows.clone();
        let rows_for_toggle = rows.clone();
        let toggle_view = view.clone();
        let select_view = view.clone();

        div()
            .w(px(280.))
            .h(px(240.))
            .panel(cx)
            .rounded(radius)
            .border_1()
            .border_color(border)
            .overflow_hidden()
            .child(
                Tree::new("demo-tree")
                    .rows(items)
                    .row_height(px(24.))
                    .indent(px(14.))
                    .selected(self.tree_selected)
                    .render_row(move |ix, _window, _cx| {
                        let row = &rows_for_render[ix];
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(if row.is_dir { dir_color } else { muted })
                                    .child(if row.is_dir { "▣" } else { "·" }),
                            )
                            .child(div().text_size(px(12.5)).child(row.label))
                            .into_any_element()
                    })
                    .on_toggle(move |ix, _window, cx| {
                        let path = rows_for_toggle[ix].path.clone();
                        toggle_view.update(cx, |this, cx| {
                            if !this.tree_expanded.remove(&path) {
                                this.tree_expanded.insert(path);
                            }
                            cx.notify();
                        });
                    })
                    .on_select(move |ix, _event, _window, cx| {
                        select_view.update(cx, |this, cx| {
                            this.tree_selected = Some(ix);
                            cx.notify();
                        });
                    }),
            )
    }

    /// The active streaming source as a trait object (synthetic or SQLite).
    fn stream_source(&self) -> Rc<dyn RowSource> {
        match (self.stream_source_ix, self.sqlite.as_ref()) {
            (1, Some(sqlite)) => sqlite.clone(),
            _ => self.synthetic.clone(),
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

    /// **M0 spike A** — a streaming grid over a bounded window buffer, with a
    /// live readout proving memory stays flat, plus an out-of-band cancel demo.
    fn streaming_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let source = self.stream_source();
        let specs = source.columns().to_vec();
        let total = source.total();
        let radius = theme.radius;
        let rownum_color = theme.text_faint;
        let muted = theme.text_muted;
        let colors = CellColors {
            text: theme.text,
            faint: theme.text_faint,
            num: theme.orange,
            json: theme.cyan,
            bool_true: theme.green,
            bool_false: theme.red,
        };

        // A row-number gutter column + one column per source column.
        let mut columns = vec![Column::new("#").width(px(64.)).align_end()];
        for spec in &specs {
            let c = Column::new(spec.name.clone());
            columns.push(if spec.numeric {
                c.width(px(120.)).align_end()
            } else {
                c.flex()
            });
        }
        let ncols = specs.len();

        let buf_for_range = self.stream_buffer.clone();
        let src_for_range = source.clone();
        let buf_for_row = self.stream_buffer.clone();

        let table = Table::<()>::new("stream-grid", columns)
            .row_count(total)
            .row_height(px(25.))
            .track_scroll(&self.stream_scroll)
            // Fill the window around the viewport *before* the rows render.
            .on_visible_range(move |range, _window, _cx| {
                buf_for_range.borrow_mut().ensure(&*src_for_range, range);
            })
            .render_row(move |ix, _window, _cx| {
                let buf = buf_for_row.borrow();
                let mut out: Vec<gpui::AnyElement> = Vec::with_capacity(ncols + 1);
                out.push(
                    div()
                        .text_color(rownum_color)
                        .child((ix + 1).to_string())
                        .into_any_element(),
                );
                match buf.get(ix) {
                    Some(cells) => {
                        for cell in cells {
                            let is_null = cell.kind == streaming::CellKind::Null;
                            out.push(
                                div()
                                    .text_color(colors.for_kind(cell.kind))
                                    .when(is_null, |d| d.italic())
                                    .child(cell.text.clone())
                                    .into_any_element(),
                            );
                        }
                    }
                    // Not yet loaded — a skeleton placeholder (M5 would shimmer).
                    None => {
                        for _ in 0..ncols {
                            out.push(div().text_color(rownum_color).child("·").into_any_element());
                        }
                    }
                }
                out
            });

        // Live buffer stats (one frame behind: paint fills the buffer after this
        // render builds the tree). Proves `buffered` plateaus while `total` is huge.
        let readout = {
            let b = self.stream_buffer.borrow();
            let fetch = b
                .last_fetch()
                .map(|r| format!("{}..{}", r.start, r.end))
                .unwrap_or_else(|| "—".into());
            let w = b.window();
            format!(
                "buffered {} rows  ·  window {}..{}  ·  last fetch {}  ·  {} fetches  ·  {} rows read  ·  total {}",
                b.buffered(),
                w.start,
                w.end,
                fetch,
                b.fetches(),
                b.rows_read(),
                total,
            )
        };

        // Source switcher.
        let view = cx.entity();
        let segmented = Segmented::new("stream-src")
            .segment("Synthetic · 10M")
            .segment("SQLite · 2M")
            .selected(self.stream_source_ix)
            .on_select(move |ix, _window, cx| {
                view.update(cx, |this, cx| {
                    if this.stream_source_ix != ix {
                        this.stream_source_ix = ix;
                        this.stream_buffer.borrow_mut().clear();
                    }
                    cx.notify();
                });
            });

        // Cancel demo.
        let slow = self.slow.lock().unwrap();
        let running = slow.status.is_running();
        let status_label = slow.status.label();
        drop(slow);
        let sqlite_note = if self.stream_source_ix == 1 && self.sqlite.is_none() {
            format!("preparing {SQLITE_ROWS}-row SQLite table in the background…")
        } else {
            String::new()
        };

        let cancel_row = div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                Button::new("slow-run", "Run slow query")
                    .variant(ButtonVariant::Secondary)
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _window, cx| {
                        streaming::run_slow(this.slow.clone());
                        let shared = this.slow.clone();
                        let bg = cx.background_executor().clone();
                        // Poll the worker's status and repaint until it settles.
                        cx.spawn(async move |this, cx| loop {
                            bg.timer(Duration::from_millis(100)).await;
                            let running = shared.lock().unwrap().status.is_running();
                            let _ = this.update(cx, |_, cx| cx.notify());
                            if !running {
                                break;
                            }
                        })
                        .detach();
                    })),
            )
            .child(
                Button::new("slow-cancel", "Cancel")
                    .variant(ButtonVariant::Danger)
                    .disabled(!running)
                    .on_click(cx.listener(|this, _, _window, cx| {
                        streaming::cancel_slow(&this.slow);
                        cx.notify();
                    })),
            )
            .child(div().text_xs().text_color(muted).child(status_label));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .w_full()
            .child(segmented)
            .child(
                div()
                    .h(px(360.))
                    .w_full()
                    .panel(cx)
                    .rounded(radius)
                    .overflow_hidden()
                    .child(table),
            )
            .child(div().text_xs().text_color(rownum_color).child(readout))
            .when(!sqlite_note.is_empty(), |this| {
                this.child(div().text_xs().text_color(theme.yellow).child(sqlite_note))
            })
            .child(cancel_row)
    }

    /// **M0 spike B** — the multiline SQL editor with live highlighting, a
    /// line-number gutter, a Run (⌘↵) affordance and a read-only toggle.
    fn sql_editor(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.text_muted;
        let yellow = theme.yellow;
        let radius_sm = theme.radius_sm;

        // Editor bar: Run button, ⌘↵ hint, read-only toggle + chip, last run.
        let toggle_view = cx.entity();
        let read_only = self.editor_readonly;
        let run_btn = Button::new("editor-run", "Run")
            .variant(ButtonVariant::Primary)
            .on_click(cx.listener(|this, _, _window, cx| {
                let sql = this.sql_editor.read(cx).content();
                this.last_run = Some(summarize_sql(&sql));
                cx.notify();
            }));

        let mut bar = div()
            .flex()
            .items_center()
            .gap_3()
            .h(px(34.))
            .px_3()
            .panel(cx)
            .child(run_btn)
            .child(div().text_xs().text_color(muted).child("⌘↵"))
            .child(
                Toggle::new("editor-ro", read_only).on_change(move |on, _window, cx| {
                    let on = *on;
                    toggle_view.update(cx, |this, cx| {
                        this.editor_readonly = on;
                        this.sql_editor
                            .update(cx, |ed, cx| ed.set_read_only(on, cx));
                        cx.notify();
                    });
                }),
            )
            .child(div().text_xs().text_color(muted).child("read-only"));

        if read_only {
            bar = bar.child(
                div()
                    .px_2()
                    .py_0p5()
                    .rounded(radius_sm)
                    .bg(yellow.opacity(0.12))
                    .text_xs()
                    .text_color(yellow)
                    .child("READ ONLY"),
            );
        }
        if let Some(last) = self.last_run.as_ref() {
            bar = bar.child(
                div()
                    .ml_auto()
                    .text_xs()
                    .text_color(muted)
                    .child(format!("ran: {last}")),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_0()
            .w_full()
            .child(self.sql_editor.clone())
            .child(bar)
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
        // Lazily open the streaming SQLite source once the background generator
        // has finished writing the temp DB.
        if self.stream_source_ix == 1
            && self.sqlite.is_none()
            && self.db_ready.load(Ordering::SeqCst)
        {
            if let Ok(src) = SqliteSource::open(&streaming::db_path()) {
                self.sqlite = Some(Rc::new(src));
                self.stream_buffer.borrow_mut().clear();
            }
        }

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
        let split_pane = self.split_pane(cx);
        let tree = self.tree(cx);
        let table = self.table(cx);
        let secondary_table = self.secondary_table(cx);
        let drag_table = self.drag_table(cx);
        let streaming_grid = self.streaming_grid(cx);
        let sql_editor = self.sql_editor(cx);

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
            .child(self.section("Split pane (nested, resizable)", split_pane, cx))
            .child(self.section("Tree (virtualized, disclosure)", tree, cx))
            .child(self.section("Table", table, cx))
            .child(self.section("Table - right-click menu", secondary_table, cx))
            .child(self.section("Table - in-app drag to folder", drag_table, cx))
            .child(self.section(
                "M0 spike A - streaming grid (bounded window + cancel)",
                streaming_grid,
                cx,
            ))
            .child(self.section(
                "M0 spike B - multiline SQL editor (syntax highlighting)",
                sql_editor,
                cx,
            ));

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

/// A node in the gallery's demo tree. A non-empty `children` makes it a folder.
struct DemoNode {
    label: &'static str,
    children: &'static [DemoNode],
}

const DEMO_TREE: &[DemoNode] = &[
    DemoNode {
        label: "src",
        children: &[
            DemoNode {
                label: "main.rs",
                children: &[],
            },
            DemoNode {
                label: "app.rs",
                children: &[],
            },
            DemoNode {
                label: "components",
                children: &[
                    DemoNode {
                        label: "button.rs",
                        children: &[],
                    },
                    DemoNode {
                        label: "table.rs",
                        children: &[],
                    },
                    DemoNode {
                        label: "tree.rs",
                        children: &[],
                    },
                ],
            },
        ],
    },
    DemoNode {
        label: "docs",
        children: &[
            DemoNode {
                label: "README.md",
                children: &[],
            },
            DemoNode {
                label: "plan.md",
                children: &[],
            },
        ],
    },
];

/// One flattened visible row: the structural `item` the tree chrome draws, plus
/// the demo payload (label, full path, folder-ness) the gallery renders + toggles.
struct FlatRow {
    item: TreeItem,
    label: &'static str,
    path: String,
    is_dir: bool,
}

/// Walk the demo tree in display order, emitting a row per visible node (a node
/// is visible when every ancestor's path is in `expanded`).
fn flatten_demo(
    nodes: &'static [DemoNode],
    depth: usize,
    prefix: &str,
    expanded: &HashSet<String>,
    out: &mut Vec<FlatRow>,
) {
    for node in nodes {
        let path = if prefix.is_empty() {
            node.label.to_string()
        } else {
            format!("{prefix}/{}", node.label)
        };
        let has_children = !node.children.is_empty();
        let is_open = expanded.contains(&path);
        out.push(FlatRow {
            item: TreeItem::new(depth, has_children, is_open),
            label: node.label,
            path: path.clone(),
            is_dir: has_children,
        });
        if has_children && is_open {
            flatten_demo(node.children, depth + 1, &path, expanded, out);
        }
    }
}

/// First meaningful (non-comment) line of a query, truncated — the "last run".
fn summarize_sql(sql: &str) -> SharedString {
    let line = sql
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with("--"))
        .unwrap_or("");
    let truncated: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        format!("{truncated}…").into()
    } else {
        truncated.into()
    }
}

fn main() {
    application().run(|cx: &mut App| {
        cx.set_global(Theme::one_dark());
        TextInput::bind_keys(cx);
        CodeEditor::bind_keys(cx);

        // Kick off generation of the 2M-row SQLite table for spike A so it's
        // ready by the time anyone switches the streaming grid to it.
        let db_ready = Arc::new(AtomicBool::new(false));
        streaming::spawn_generate(db_ready.clone());

        let bounds = Bounds::centered(None, gpui::size(gpui::px(960.0), gpui::px(720.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_, cx| cx.new(|cx| Gallery::new(cx, db_ready.clone())),
        )
        .expect("failed to open gallery window");
        cx.activate(true);
    });
}
