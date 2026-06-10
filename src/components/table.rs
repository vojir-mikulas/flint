// SPDX-License-Identifier: GPL-3.0-or-later

//! `Table` - a virtualized, fixed-row-height data table on GPUI's
//! [`uniform_list`](gpui::uniform_list). Fully generic and stateless: the caller
//! declares [`Column`]s + a row renderer and owns selection/sort, which the table
//! renders and reports clicks against.

use std::ops::Range;
use std::rc::Rc;

use gpui::{
    canvas, div, point, prelude::*, uniform_list, App, Bounds, ClickEvent, DispatchPhase,
    ExternalPaths, IsZero, MouseButton, Pixels, Point, ScrollHandle, ScrollWheelEvent,
    SharedString, Styled, UniformListScrollHandle, Window,
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

/// A rectangular cell selection, `anchor` → `focus` (either corner may lead).
/// Caller-owned, like row selection: the table highlights cells inside it and
/// reports clicks via [`Table::on_cell_click`]; copy-as-TSV is the caller's to
/// assemble from its own data over [`Self::bounds`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CellRange {
    pub anchor: (usize, usize),
    pub focus: (usize, usize),
}

impl CellRange {
    /// A 1×1 selection at `(row, col)`.
    pub fn single(row: usize, col: usize) -> Self {
        Self {
            anchor: (row, col),
            focus: (row, col),
        }
    }

    /// Normalized `(row0, col0, row1, col1)`, inclusive, with `0 <= 0-corner`.
    pub fn bounds(&self) -> (usize, usize, usize, usize) {
        (
            self.anchor.0.min(self.focus.0),
            self.anchor.1.min(self.focus.1),
            self.anchor.0.max(self.focus.0),
            self.anchor.1.max(self.focus.1),
        )
    }

    pub fn contains(&self, row: usize, col: usize) -> bool {
        let (r0, c0, r1, c1) = self.bounds();
        (r0..=r1).contains(&row) && (c0..=c1).contains(&col)
    }
}

#[derive(Clone)]
pub struct Column {
    title: SharedString,
    /// Dimmer secondary text after the title in the header (e.g. a column type).
    subtitle: Option<SharedString>,
    width: ColumnWidth,
    align: ColumnAlign,
    sortable: bool,
}

impl Column {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            subtitle: None,
            width: ColumnWidth::default(),
            align: ColumnAlign::default(),
            sortable: false,
        }
    }

    /// A dimmer secondary label rendered after the title in the header — the
    /// design's typed column headers (`email` + `text`).
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
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
/// Cell-click handler `(row, col, click)`; the click's modifiers drive whether
/// the caller resets or extends its [`CellRange`] (shift-click extends).
type CellClickHandler = Rc<dyn Fn(usize, usize, &ClickEvent, &mut Window, &mut App) + 'static>;
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
    h_scroll_handle: Option<ScrollHandle>,
    on_visible_range: Option<VisibleRangeHandler>,
    selected_cells: Option<CellRange>,
    on_cell_click: Option<CellClickHandler>,
    horizontal: bool,
    /// Font family for the header + cells (e.g. a monospace data grid). `None`
    /// inherits the ambient family.
    font_family: Option<SharedString>,
    /// Draw 1px separators between cells and rows (a spreadsheet look).
    grid_lines: bool,
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
            h_scroll_handle: None,
            on_visible_range: None,
            selected_cells: None,
            on_cell_click: None,
            horizontal: false,
            font_family: None,
            grid_lines: false,
        }
    }

    /// Render the header + cells in `family` (e.g. a monospace data grid).
    pub fn font_family(mut self, family: impl Into<SharedString>) -> Self {
        self.font_family = Some(family.into());
        self
    }

    /// Draw 1px separators between cells and rows for a spreadsheet look.
    pub fn grid_lines(mut self, grid_lines: bool) -> Self {
        self.grid_lines = grid_lines;
        self
    }

    /// The current cell selection to highlight (caller-owned).
    pub fn selected_cells(mut self, range: Option<CellRange>) -> Self {
        self.selected_cells = range;
        self
    }

    /// Per-cell click handler. The click's modifiers let the caller extend
    /// (shift-click) vs. reset the [`CellRange`].
    pub fn on_cell_click(
        mut self,
        handler: impl Fn(usize, usize, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_cell_click = Some(Rc::new(handler));
        self
    }

    /// Lay columns out at their fixed widths and scroll horizontally (header and
    /// rows together) when they overflow — for wide results. Columns should carry
    /// fixed widths; flex columns get a default width in this mode.
    pub fn horizontal(mut self, horizontal: bool) -> Self {
        self.horizontal = horizontal;
        self
    }

    /// Bind the list's scroll position to a caller-owned handle, so the owner can
    /// read the offset and scroll programmatically (e.g. rubber-band auto-scroll).
    pub fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.scroll_handle = Some(handle.clone());
        self
    }

    /// Bind the horizontal (wide-mode) scroll position to a caller-owned handle.
    /// Required for the trackpad axis-lock in [`horizontal`](Self::horizontal)
    /// mode: with it bound, a dominantly-horizontal swipe won't bleed into
    /// vertical drift (and vice versa), while true diagonal swipes still move
    /// both axes. The handle must outlive a single render (store it on the view).
    pub fn track_horizontal_scroll(mut self, handle: &ScrollHandle) -> Self {
        self.h_scroll_handle = Some(handle.clone());
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

        let grid_lines = self.grid_lines;
        let line = theme.border_soft;
        let font_family = self.font_family.clone();

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
            let color = if sorted { theme.text } else { theme.text_muted };
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
                .when_some(column.subtitle.clone(), |this, sub| {
                    this.child(
                        div()
                            .text_size(gpui::px(10.))
                            .text_color(theme.text_faint)
                            .child(sub),
                    )
                })
                .when_some(caret, |this, caret| this.child(caret));
            let cell = cell_layout(cell, column, column.align)
                .when(grid_lines, |c| c.border_r_1().border_color(line));

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
            .when_some(font_family.clone(), |d, f| d.font_family(f))
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
        let cell_selected = theme.accent_ghost;
        let text = theme.text;

        let selected_cells = self.selected_cells;
        let on_cell_click = self.on_cell_click.clone();

        let list = uniform_list("table-rows", row_count, move |range, window, cx| {
            // Report the about-to-render window so a windowed source can fill the
            // buffer (and evict outside it) before `render_row` reads it below.
            //
            // `uniform_list` also re-renders a single item (`0..1`) every frame
            // just to *measure* row size. That call is indistinguishable from a
            // real viewport except by length — forwarding it would feed the
            // caller's windowed source a degenerate range each frame, evicting
            // its buffer and corrupting any scroll-velocity tracking. So only
            // multi-row (genuine viewport) ranges pass through; a table with a
            // single row still reports, since `0..1` is then the real viewport.
            if let Some(on_visible_range) = on_visible_range.as_ref() {
                if range.len() > 1 || row_count <= 1 {
                    on_visible_range(range.clone(), window, cx);
                }
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
                    let is_cell_selected =
                        selected_cells.is_some_and(|range| range.contains(ix, c));
                    let on_cell_click = on_cell_click.clone();
                    cell_layout(
                        div()
                            .id(SharedString::from(format!("cell-{ix}-{c}")))
                            .flex()
                            .items_center()
                            .h_full()
                            .px_2p5()
                            .overflow_hidden()
                            .when(grid_lines, |d| d.border_r_1().border_color(line))
                            .when(is_cell_selected, |d| d.bg(cell_selected))
                            .when_some(on_cell_click, |d, handler| {
                                d.cursor_pointer().on_click(move |event, window, cx| {
                                    handler(ix, c, event, window, cx)
                                })
                            })
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
                        .when(grid_lines, |this| this.border_b_1().border_color(line))
                        .when_some(font_family.clone(), |this, f| this.font_family(f))
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

        // Wide mode: header + rows share one horizontally-scrolling, fixed-width
        // track, so they scroll in lockstep while the list still virtualizes
        // vertically. Otherwise columns flex to fit and there's no x-scroll.
        if self.horizontal {
            let total: f32 = self
                .columns
                .iter()
                .map(|c| match c.width {
                    ColumnWidth::Fixed(w) => f32::from(w),
                    ColumnWidth::Flex => 160.,
                })
                .sum();
            let mut hscroll = div()
                .id("table-hscroll")
                .flex_1()
                .min_h(gpui::px(0.))
                .overflow_x_scroll();
            if let Some(h) = self.h_scroll_handle.as_ref() {
                hscroll = hscroll.track_scroll(h);
            }
            let mut hscroll = hscroll.child(
                div()
                    .flex()
                    .flex_col()
                    // Fixed to the columns' combined width so rows + header scroll
                    // in lockstep — but at least the viewport's width, so when the
                    // columns are narrower than the pane the rows still fill it.
                    // Otherwise the blank strip beside the columns sits outside the
                    // list, and a vertical wheel there lands on the x-only scroll
                    // container and does nothing.
                    .w(gpui::px(total))
                    .min_w(gpui::relative(1.))
                    .h_full()
                    .child(header)
                    .child(list),
            );
            // Keep a pure-vertical wheel from being redirected into x-scroll.
            hscroll.style().restrict_scroll_to_axis = Some(true);

            let mut root = div().id(self.id).flex().flex_col().size_full();

            // The horizontal track and the vertical `uniform_list` are two
            // independent scroll containers, so GPUI's per-container axis lock
            // never sees both axes at once — a trackpad swipe's minor-axis jitter
            // leaks into the other container and the grid drifts diagonally. This
            // capture-phase overlay arbitrates across both: it picks the dominant
            // axis per scroll event and, when one clearly dominates, drives only
            // that handle and swallows the event; a true diagonal (within 2:1)
            // falls through so the native handlers still move both axes.
            if let (Some(h), Some(v)) = (self.h_scroll_handle.clone(), self.scroll_handle.clone()) {
                let v = v.0.borrow().base_handle.clone();
                root = root.relative().child(
                    canvas(
                        |_, _, _| (),
                        move |bounds: Bounds<Pixels>, _, window, _| {
                            let view = window.current_view();
                            let line_height = window.line_height();
                            window.on_mouse_event(
                                move |event: &ScrollWheelEvent, phase, _window, cx| {
                                    if phase != DispatchPhase::Capture
                                        || !bounds.contains(&event.position)
                                    {
                                        return;
                                    }
                                    let delta = event.delta.pixel_delta(line_height);
                                    let (ax, ay) = (delta.x.abs(), delta.y.abs());
                                    // A clean single-axis wheel falls through to
                                    // the native handlers untouched.
                                    if ax.is_zero() || ay.is_zero() {
                                        return;
                                    }
                                    const LOCK_RATIO: f32 = 2.0;
                                    if ax > ay * LOCK_RATIO {
                                        h.set_offset(h.offset() + point(delta.x, gpui::px(0.)));
                                    } else if ay > ax * LOCK_RATIO {
                                        v.set_offset(v.offset() + point(gpui::px(0.), delta.y));
                                    } else {
                                        return; // diagonal: let both axes move
                                    }
                                    cx.stop_propagation();
                                    cx.notify(view);
                                },
                            );
                        },
                    )
                    .absolute()
                    .size_full(),
                );
            }

            root.child(hscroll)
        } else {
            div()
                .id(self.id)
                .flex()
                .flex_col()
                .size_full()
                .child(header)
                .child(list)
        }
    }
}
