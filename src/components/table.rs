// SPDX-License-Identifier: GPL-3.0-or-later

//! `Table` - a virtualized, fixed-row-height data table on GPUI's
//! [`uniform_list`](gpui::uniform_list). Fully generic and stateless: the caller
//! declares [`Column`]s + a row renderer and owns selection/sort, which the table
//! renders and reports clicks against.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    canvas, div, prelude::*, uniform_list, App, Bounds, ClickEvent, ExternalPaths, MouseButton,
    Pixels, Point, SharedString, Styled, UniformListScrollHandle, Window,
};

use crate::theme::ActiveTheme;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum ColumnWidth {
    /// Shares leftover space equally with other flex columns.
    #[default]
    Flex,
    Fixed(Pixels),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColumnAlign {
    #[default]
    Start,
    End,
}

#[derive(Clone)]
pub struct Column {
    title: SharedString,
    width: ColumnWidth,
    align: ColumnAlign,
    sortable: bool,
}

impl Column {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            width: ColumnWidth::default(),
            align: ColumnAlign::default(),
            sortable: false,
        }
    }

    pub fn width(mut self, width: Pixels) -> Self {
        self.width = ColumnWidth::Fixed(width);
        self
    }

    pub fn flex(mut self) -> Self {
        self.width = ColumnWidth::Flex;
        self
    }

    pub fn align_end(mut self) -> Self {
        self.align = ColumnAlign::End;
        self
    }

    pub fn sortable(mut self) -> Self {
        self.sortable = true;
        self
    }
}

fn cell_layout<E: Styled>(el: E, column: &Column, align: ColumnAlign) -> E {
    let el = match column.width {
        ColumnWidth::Fixed(w) => el.w(w).flex_shrink_0(),
        ColumnWidth::Flex => el.flex_1().min_w_0(),
    };
    match align {
        ColumnAlign::Start => el.justify_start(),
        ColumnAlign::End => el.justify_end(),
    }
}

type IndexHandler = Box<dyn Fn(usize, &mut Window, &mut App) + 'static>;
/// Reports the currently visible row range on every paint, *before* the rows in
/// it are rendered. Lets a caller back the table with a windowed/streaming data
/// source: prefetch the window around the viewport and evict everything else, so
/// memory stays bounded no matter how many rows the source claims to have.
type VisibleRangeHandler = Rc<dyn Fn(Range<usize>, &mut Window, &mut App) + 'static>;
/// Row-click handler; also receives the click, for modifier-aware selection.
type RowClickHandler = Box<dyn Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static>;
/// Receives the row index and cursor position, to anchor a context menu.
type RowSecondaryHandler = Box<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static>;
/// Builds one cell [`AnyElement`] per column for a row.
type RowRenderer = Rc<dyn Fn(usize, &mut Window, &mut App) -> Vec<gpui::AnyElement> + 'static>;
/// Builds the sort caret. Returns an [`AnyElement`] so the library stays
/// domain- and icon-set-free.
type CaretBuilder = Rc<dyn Fn() -> gpui::AnyElement + 'static>;
type RowDropHandler = Rc<dyn Fn(usize, &ExternalPaths, &mut Window, &mut App) + 'static>;
/// Produces the in-app drag payload for a row, or `None` if it isn't draggable.
/// The payload type `D` is the caller's; the table stays domain-agnostic.
type RowDragValue<D> = Rc<dyn Fn(usize) -> Option<D> + 'static>;
/// Builds the floating preview shown under the cursor while a row is dragged.
/// Keyed on the row index so it needs no knowledge of the payload type.
type DragPreviewBuilder = Rc<dyn Fn(usize, &mut Window, &mut App) -> gpui::AnyElement + 'static>;
/// Handles an in-app payload `D` dropped onto a row.
type RowDropItemHandler<D> = Rc<dyn Fn(usize, &D, &mut Window, &mut App) + 'static>;
/// Reports a row's painted rect (window coordinates) on every paint, for hit
/// testing a drop that the platform can't route through GPUI (e.g. an OS
/// drag-out returning inside the window).
type RowBoundsHandler = Rc<dyn Fn(usize, Bounds<Pixels>, &mut Window, &mut App) + 'static>;

type PreviewFn = Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyElement + 'static>;
/// Per-row boolean predicate (selected / draggable / droppable / highlighted).
/// Queried only for visible rows, so it stays O(1) even for huge listings - the
/// caller never materializes a set spanning every row.
type RowPredicate = Rc<dyn Fn(usize) -> bool + 'static>;

/// Wraps a caller-built element as the floating in-app drag preview view -
/// GPUI's `on_drag` requires an `Entity<impl Render>`, so we box the builder.
struct DragPreview {
    build: PreviewFn,
}

impl Render for DragPreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.build)(window, cx)
    }
}

/// `D` is the in-app drag payload type a row produces and a drop target
/// receives. It defaults to `()` for tables that don't use in-app drag.
#[derive(IntoElement)]
pub struct Table<D: 'static = ()> {
    id: SharedString,
    columns: Rc<Vec<Column>>,
    row_count: usize,
    row_height: Option<Pixels>,
    selected: Option<usize>,
    selected_set: Option<RowPredicate>,
    sort: Option<(usize, bool)>,
    on_select: Option<Rc<RowClickHandler>>,
    on_secondary: Option<Rc<RowSecondaryHandler>>,
    on_activate: Option<Rc<IndexHandler>>,
    on_sort: Option<Rc<IndexHandler>>,
    render_row: Option<RowRenderer>,
    sort_caret_asc: Option<CaretBuilder>,
    sort_caret_desc: Option<CaretBuilder>,
    on_row_drop: Option<RowDropHandler>,
    droppable_rows: Option<RowPredicate>,
    on_row_drag: Option<RowDragValue<D>>,
    drag_preview: Option<DragPreviewBuilder>,
    on_row_drop_item: Option<RowDropItemHandler<D>>,
    on_row_bounds: Option<RowBoundsHandler>,
    highlighted_rows: Option<RowPredicate>,
    draggable_rows: Option<RowPredicate>,
    scroll_handle: Option<UniformListScrollHandle>,
    on_visible_range: Option<VisibleRangeHandler>,
}

impl<D: 'static> Table<D> {
    pub fn new(id: impl Into<SharedString>, columns: Vec<Column>) -> Self {
        Self {
            id: id.into(),
            columns: Rc::new(columns),
            row_count: 0,
            row_height: None,
            selected: None,
            selected_set: None,
            sort: None,
            on_select: None,
            on_secondary: None,
            on_activate: None,
            on_sort: None,
            render_row: None,
            sort_caret_asc: None,
            sort_caret_desc: None,
            on_row_drop: None,
            droppable_rows: None,
            on_row_drag: None,
            drag_preview: None,
            on_row_drop_item: None,
            on_row_bounds: None,
            highlighted_rows: None,
            draggable_rows: None,
            scroll_handle: None,
            on_visible_range: None,
        }
    }

    /// Bind the list's scroll position to a caller-owned handle, so the owner can
    /// read the offset and scroll programmatically (e.g. rubber-band auto-scroll).
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    pub fn row_count(mut self, row_count: usize) -> Self {
        self.row_count = row_count;
        self
    }

    /// Defaults to the theme's `row_height`.
    pub fn row_height(mut self, height: Pixels) -> Self {
        self.row_height = Some(height);
        self
    }

    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }

    /// Multi-selection: a row highlights when this predicate returns `true` *or*
    /// its index equals [`selected`](Self::selected), so both APIs compose. The
    /// predicate is queried only for visible rows.
    pub fn selected_set(mut self, is_selected: impl Fn(usize) -> bool + 'static) -> Self {
        self.selected_set = Some(Rc::new(is_selected));
        self
    }

    /// `(column_index, ascending)`, to draw the caret.
    pub fn sort(mut self, sort: Option<(usize, bool)>) -> Self {
        self.sort = sort;
        self
    }

    pub fn on_select(
        mut self,
        handler: impl Fn(usize, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_select = Some(Rc::new(Box::new(handler)));
        self
    }

    pub fn on_secondary(
        mut self,
        handler: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_secondary = Some(Rc::new(Box::new(handler)));
        self
    }

    /// Double-click; does not also fire [`on_select`](Self::on_select).
    pub fn on_activate(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_activate = Some(Rc::new(Box::new(handler)));
        self
    }

    pub fn on_sort(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_sort = Some(Rc::new(Box::new(handler)));
        self
    }

    /// Caret glyphs for the active sort column. Unset falls back to built-in
    /// Unicode triangles.
    pub fn sort_carets(
        mut self,
        asc: impl Fn() -> gpui::AnyElement + 'static,
        desc: impl Fn() -> gpui::AnyElement + 'static,
    ) -> Self {
        self.sort_caret_asc = Some(Rc::new(asc));
        self.sort_caret_desc = Some(Rc::new(desc));
        self
    }

    /// Rows for which this predicate returns `true` highlight on drag-over and
    /// dispatch [`on_row_drop`](Self::on_row_drop) (the owner knows which are
    /// directories). Queried only for visible rows.
    pub fn droppable_rows(mut self, is_droppable: impl Fn(usize) -> bool + 'static) -> Self {
        self.droppable_rows = Some(Rc::new(is_droppable));
        self
    }

    pub fn on_row_drop(
        mut self,
        handler: impl Fn(usize, &ExternalPaths, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_row_drop = Some(Rc::new(handler));
        self
    }

    /// Rows for which this predicate returns `true` start an in-app drag gesture
    /// (the owner decides which are draggable). Queried only for visible rows.
    pub fn draggable_rows(mut self, is_draggable: impl Fn(usize) -> bool + 'static) -> Self {
        self.draggable_rows = Some(Rc::new(is_draggable));
        self
    }

    /// Produces the in-app drag payload for a [`draggable`](Self::draggable_rows)
    /// row, or `None` to skip the gesture. The payload flows to a row's
    /// [`on_row_drop_item`](Self::on_row_drop_item).
    pub fn on_row_drag(mut self, handler: impl Fn(usize) -> Option<D> + 'static) -> Self {
        self.on_row_drag = Some(Rc::new(handler));
        self
    }

    /// Builds the floating preview shown under the cursor while a row is dragged.
    pub fn drag_preview(
        mut self,
        builder: impl Fn(usize, &mut Window, &mut App) -> gpui::AnyElement + 'static,
    ) -> Self {
        self.drag_preview = Some(Rc::new(builder));
        self
    }

    /// Accept an in-app payload `D` dropped onto a [`droppable`](Self::droppable_rows)
    /// row. Composes with the [`ExternalPaths`] [`on_row_drop`](Self::on_row_drop).
    pub fn on_row_drop_item(
        mut self,
        handler: impl Fn(usize, &D, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_row_drop_item = Some(Rc::new(handler));
        self
    }

    /// Report each visible row's painted rect (window coordinates) on every
    /// paint. Lets the owner hit-test a drop the platform can't deliver through
    /// GPUI's normal drop path.
    pub fn on_row_bounds(
        mut self,
        handler: impl Fn(usize, Bounds<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_row_bounds = Some(Rc::new(handler));
        self
    }

    /// Rows for which this predicate returns `true` paint with the drop-target
    /// highlight, independent of any active GPUI drag. Used to show a target for a
    /// platform drag GPUI can't observe. Queried only for visible rows.
    pub fn highlighted_rows(mut self, is_highlighted: impl Fn(usize) -> bool + 'static) -> Self {
        self.highlighted_rows = Some(Rc::new(is_highlighted));
        self
    }

    pub fn render_row(
        mut self,
        renderer: impl Fn(usize, &mut Window, &mut App) -> Vec<gpui::AnyElement> + 'static,
    ) -> Self {
        self.render_row = Some(Rc::new(renderer));
        self
    }

    /// Called once per paint with the row range `uniform_list` is about to
    /// render, before [`render_row`](Self::render_row) runs for any row in it.
    /// A caller backing the table with a windowed source uses this to prefetch
    /// the visible window (and drop rows outside it) so [`render_row`](Self::render_row)
    /// then hits an already-populated buffer. Stays domain-free: the table knows
    /// nothing about where rows come from.
    pub fn on_visible_range(
        mut self,
        handler: impl Fn(Range<usize>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_visible_range = Some(Rc::new(handler));
        self
    }
}

impl<D: 'static> RenderOnce for Table<D> {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let row_height = self.row_height.unwrap_or(theme.row_height);
        let columns = self.columns.clone();
        let sort = self.sort;

        let on_sort = self.on_sort.clone();
        let caret_asc = self.sort_caret_asc.clone();
        let caret_desc = self.sort_caret_desc.clone();
        let header_cells = columns.iter().enumerate().map(|(ix, column)| {
            let sorted = sort.map(|(c, _)| c == ix).unwrap_or(false);
            // Caller-supplied caret glyph if set, else the built-in triangle.
            let caret: Option<gpui::AnyElement> = match sort {
                Some((c, asc)) if c == ix => Some(if asc {
                    caret_asc
                        .as_ref()
                        .map(|f| f())
                        .unwrap_or_else(|| div().text_xs().child("▲").into_any_element())
                } else {
                    caret_desc
                        .as_ref()
                        .map(|f| f())
                        .unwrap_or_else(|| div().text_xs().child("▼").into_any_element())
                }),
                _ => None,
            };
            let color = if sorted {
                theme.text_muted
            } else {
                theme.text_faint
            };
            let on_sort = on_sort.clone();

            let cell = div()
                .id(ix)
                .flex()
                .items_center()
                .gap_1()
                .h_full()
                .px_2p5()
                .text_color(color)
                .child(column.title.clone())
                .when_some(caret, |this, caret| this.child(caret));
            let cell = cell_layout(cell, column, column.align);

            if column.sortable {
                cell.cursor_pointer()
                    .hover(|s| s.text_color(theme.text))
                    .when_some(on_sort, |this, on_sort| {
                        this.on_click(move |_, window, cx| on_sort(ix, window, cx))
                    })
                    .into_any_element()
            } else {
                cell.into_any_element()
            }
        });

        let header = div()
            .id("table-head")
            .flex()
            .items_center()
            .h(gpui::px(28.))
            .border_b_1()
            .border_color(theme.border_soft)
            .text_xs()
            .children(header_cells);

        let columns_for_rows = columns.clone();
        let render_row = self.render_row.clone();
        let on_select = self.on_select.clone();
        let on_secondary = self.on_secondary.clone();
        let on_activate = self.on_activate.clone();
        let on_row_drop = self.on_row_drop.clone();
        let droppable_rows = self.droppable_rows.clone();
        let on_row_drag = self.on_row_drag.clone();
        let drag_preview = self.drag_preview.clone();
        let on_row_drop_item = self.on_row_drop_item.clone();
        let on_row_bounds = self.on_row_bounds.clone();
        let highlighted_rows = self.highlighted_rows.clone();
        let draggable_rows = self.draggable_rows.clone();
        let selected = self.selected;
        let selected_set = self.selected_set.clone();
        let on_visible_range = self.on_visible_range.clone();
        let row_count = self.row_count;

        // Token snapshot so the `'static` row closure doesn't borrow `cx`.
        let bg_hover = theme.bg_hover;
        let bg_selected = theme.bg_selected;
        let drop_highlight = theme.bg_active;
        let text = theme.text;

        let list = uniform_list("table-rows", row_count, move |range, window, cx| {
            // Report the about-to-render window so a windowed source can fill the
            // buffer (and evict outside it) before `render_row` reads it below.
            if let Some(on_visible_range) = on_visible_range.as_ref() {
                on_visible_range(range.clone(), window, cx);
            }
            let mut rows = Vec::with_capacity(range.len());
            for ix in range {
                let is_selected =
                    selected == Some(ix) || selected_set.as_ref().is_some_and(|f| f(ix));
                let is_highlighted = highlighted_rows.as_ref().is_some_and(|f| f(ix));
                let cells = render_row
                    .as_ref()
                    .map(|r| r(ix, window, cx))
                    .unwrap_or_default();

                let laid_out = cells.into_iter().enumerate().map(|(c, cell)| {
                    let column = &columns_for_rows[c];
                    cell_layout(
                        div()
                            .flex()
                            .items_center()
                            .h_full()
                            .px_2p5()
                            .overflow_hidden()
                            .child(cell),
                        column,
                        column.align,
                    )
                });

                let on_select = on_select.clone();
                let on_secondary = on_secondary.clone();
                let on_activate = on_activate.clone();
                let on_row_drop = on_row_drop.clone();
                let on_row_drop_item = on_row_drop_item.clone();
                let on_row_drag = on_row_drag.clone();
                let drag_preview = drag_preview.clone();
                let on_row_bounds = on_row_bounds.clone();
                let clickable = on_select.is_some() || on_activate.is_some();
                let is_droppable =
                    droppable_rows.as_ref().is_some_and(|f| f(ix)) && on_row_drop.is_some();
                let is_droppable_item =
                    droppable_rows.as_ref().is_some_and(|f| f(ix)) && on_row_drop_item.is_some();
                let is_draggable =
                    draggable_rows.as_ref().is_some_and(|f| f(ix)) && on_row_drag.is_some();
                rows.push(
                    div()
                        .id(ix)
                        .flex()
                        .items_center()
                        // `uniform_list` lays each row out as a layout root; `w_full`
                        // makes it fill the width so flex columns align with the header.
                        .w_full()
                        .h(row_height)
                        .text_color(text)
                        .when(is_selected, |this| this.bg(bg_selected))
                        .when(!is_selected, |this| this.hover(move |s| s.bg(bg_hover)))
                        // A forced drop-target highlight (e.g. a platform drag GPUI
                        // can't observe) wins over selection/hover.
                        .when(is_highlighted, |this| this.bg(drop_highlight))
                        .when(clickable || on_secondary.is_some(), |this| {
                            this.cursor_pointer()
                        })
                        .when(clickable, |this| {
                            this.on_click(move |event, window, cx| {
                                if event.click_count() >= 2 {
                                    if let Some(on_activate) = on_activate.as_ref() {
                                        on_activate(ix, window, cx);
                                        return;
                                    }
                                }
                                if let Some(on_select) = on_select.as_ref() {
                                    on_select(ix, event, window, cx);
                                }
                            })
                        })
                        .when_some(on_secondary, |this, on_secondary| {
                            this.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                                on_secondary(ix, event.position, window, cx);
                            })
                        })
                        .when(is_droppable, |this| {
                            let this = this
                                .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_highlight));
                            this.when_some(on_row_drop, |this, on_row_drop| {
                                this.on_drop::<ExternalPaths>(move |paths, window, cx| {
                                    on_row_drop(ix, paths, window, cx);
                                })
                            })
                        })
                        // A row also accepts an in-app `D` drop (e.g. a move into
                        // this folder), highlighting the same as an external drop.
                        .when(is_droppable_item, |this| {
                            let this = this.drag_over::<D>(move |s, _, _, _| s.bg(drop_highlight));
                            this.when_some(on_row_drop_item, |this, on_row_drop_item| {
                                this.on_drop::<D>(move |value, window, cx| {
                                    on_row_drop_item(ix, value, window, cx);
                                })
                            })
                        })
                        // Start an in-app drag: the handler mints the payload `D`
                        // and the caller's `drag_preview` builds the cursor chip.
                        .when(is_draggable, |this| {
                            match on_row_drag.as_ref().and_then(|f| f(ix)) {
                                Some(value) => {
                                    let drag_preview = drag_preview.clone();
                                    this.on_drag(value, move |_value, offset, _window, cx| {
                                        let drag_preview = drag_preview.clone();
                                        cx.new(move |_| DragPreview {
                                            build: Box::new(move |window, cx| {
                                                let chip = drag_preview
                                                    .as_ref()
                                                    .map(|f| f(ix, window, cx))
                                                    .unwrap_or_else(|| {
                                                        div().size_0().into_any_element()
                                                    });
                                                // GPUI anchors the preview at the
                                                // row's origin (mouse - grab offset);
                                                // shift it back under the cursor so it
                                                // tracks the pointer wherever the drag
                                                // began in the row.
                                                div()
                                                    .pl(offset.x)
                                                    .pt(offset.y)
                                                    .child(chip)
                                                    .into_any_element()
                                            }),
                                        })
                                    })
                                }
                                None => this,
                            }
                        })
                        .children(laid_out)
                        // An overlay canvas reports the row's painted rect (it has
                        // no hitbox, so it doesn't intercept clicks or drops).
                        .when_some(on_row_bounds, |this, cb| {
                            this.relative().child(
                                canvas(
                                    |_bounds, _window, _cx| (),
                                    move |bounds, _, window, cx| cb(ix, bounds, window, cx),
                                )
                                .absolute()
                                .top_0()
                                .left_0()
                                .size_full(),
                            )
                        }),
                );
            }
            rows
        })
        .flex_1();
        let list = match self.scroll_handle.as_ref() {
            Some(handle) => list.track_scroll(handle),
            None => list,
        };

        div()
            .id(self.id)
            .flex()
            .flex_col()
            .size_full()
            .child(header)
            .child(list)
    }
}
