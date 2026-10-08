use std::{ops::Range, rc::Rc, time::Duration};

use crate::{
    ActiveTheme, ElementExt, Icon, IconName, StyleSized as _, StyledExt, VirtualListScrollHandle,
    actions::{
        Cancel, SelectDown, SelectFirst, SelectLast, SelectNextColumn, SelectPageDown,
        SelectPageUp, SelectPrevColumn, SelectUp,
    },
    h_flex,
    menu::{ContextMenuExt, PopupMenu},
    scroll::{ScrollableMask, Scrollbar},
    v_flex,
};
use gpui::{
    AppContext, Axis, Bounds, ClickEvent, Context, Div, DragMoveEvent, EventEmitter, FocusHandle,
    Focusable, InteractiveElement, IntoElement, ListSizingBehavior, MouseButton, MouseDownEvent,
    ParentElement, Pixels, Point, Render, ScrollStrategy, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Task, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder, px, uniform_list,
};

use super::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SelectionMode {
    Column,
    Row,
    Cell,
}

impl SelectionMode {
    #[inline(always)]
    fn is_row(&self) -> bool {
        matches!(self, SelectionMode::Row)
    }

    #[inline(always)]
    fn is_column(&self) -> bool {
        matches!(self, SelectionMode::Column)
    }

    #[inline(always)]
    fn is_cell(&self) -> bool {
        matches!(self, SelectionMode::Cell)
    }
}

/// The Table event.
#[derive(Clone)]
pub enum TableEvent {
    /// Single click or move to selected row.
    SelectRow(usize),
    /// Double click on the row.
    DoubleClickedRow(usize),
    /// Selected column.
    SelectColumn(usize),
    /// A cell has been selected (clicked or navigated to via keyboard).
    ///
    /// Emitted when a cell is selected in cell selection mode.
    /// The first `usize` is the row index, and the second `usize` is the column index.
    ///
    /// This event is also emitted when navigating between cells using keyboard shortcuts.
    SelectCell(usize, usize),
    /// A cell has been double-clicked.
    ///
    /// Emitted when a cell is double-clicked in cell selection mode.
    /// The first `usize` is the row index, and the second `usize` is the column index.
    ///
    /// Use this event to trigger actions like opening a detail view or editing the cell content.
    DoubleClickedCell(usize, usize),
    /// The column widths have changed.
    ///
    /// The `Vec<Pixels>` contains the new widths of all columns.
    ColumnWidthsChanged(Vec<Pixels>),
    /// A column has been moved.
    ///
    /// The first `usize` is the original index of the column,
    /// and the second `usize` is the new index of the column.
    MoveColumn(usize, usize),
    /// A row has been right-clicked.
    ///
    /// Contains the row index, or `None` if right-clicked on an empty area.
    /// Use this event to show context menus for rows.
    RightClickedRow(Option<usize>),
    /// A cell has been right-clicked.
    ///
    /// Emitted when a cell is right-clicked in cell selection mode.
    /// The first `usize` is the row index, and the second `usize` is the column index.
    ///
    /// Use this event to show context menus specific to the cell content.
    /// The right-clicked cell is highlighted with a subtle border until another cell is clicked.
    RightClickedCell(usize, usize),
    /// The selection has been cleared.
    ///
    /// This event is emitted when the selection is cleared.
    ClearSelection,
}

/// The visible range of the rows and columns.
#[derive(Debug, Default)]
pub struct TableVisibleRange {
    /// The visible range of the rows.
    rows: Range<usize>,
    /// The visible range of the columns.
    cols: Range<usize>,
}

impl TableVisibleRange {
    /// Returns the visible range of the rows.
    pub fn rows(&self) -> &Range<usize> {
        &self.rows
    }

    /// Returns the visible range of the columns.
    pub fn cols(&self) -> &Range<usize> {
        &self.cols
    }
}

/// The state for [`DataTable`].
///
/// # Selection Modes
///
/// The table supports three selection modes:
/// - **Row Selection**: Select entire rows (default mode)
/// - **Column Selection**: Select entire columns
/// - **Cell Selection**: Select individual cells
///
/// ## Cell Selection
///
/// When `cell_selectable` is enabled, users can:
/// - Click on cells to select them
/// - Right-click on cells to mark them for context menus
/// - Double-click on cells to trigger actions
/// - Navigate between cells using keyboard (arrow keys, Home, End, PageUp, PageDown, Tab)
///
/// When in cell selection mode, a row header column appears on the left side,
/// allowing users to select entire rows by clicking on it.
///
/// # Events
///
/// The table emits the following events related to cell selection:
/// - [`TableEvent::SelectCell`]: Emitted when a cell is selected
/// - [`TableEvent::DoubleClickedCell`]: Emitted when a cell is double-clicked
/// - [`TableEvent::RightClickedCell`]: Emitted when a cell is right-clicked
///
/// # Example
///
/// ```rust,ignore
/// let table_state = cx.new(|cx| {
///     TableState::new(delegate, cx)
///         .cell_selectable(true)
///         .row_selectable(true)
/// });
///
/// // Subscribe to cell events
/// cx.subscribe(&table_state, |this, table, event, cx| {
///     match event {
///         TableEvent::SelectCell(row_ix, col_ix) => {
///             println!("Selected cell: ({}, {})", row_ix, col_ix);
///         }
///         TableEvent::DoubleClickedCell(row_ix, col_ix) => {
///             println!("Double-clicked cell: ({}, {})", row_ix, col_ix);
///         }
///         _ => {}
///     }
/// });
/// ```
#[derive(Clone)]
pub(crate) struct HeaderCell {
    pub label: SharedString,
    pub width: Pixels,
    col_span: usize,
    is_leaf: bool,
    leaf_col_ix: Option<usize>,
    start_leaf_col_ix: usize,
}

pub struct TableState<D: TableDelegate> {
    focus_handle: FocusHandle,
    delegate: D,
    pub(super) options: TableOptions,
    /// The bounds of the table container.
    bounds: Bounds<Pixels>,
    /// The bounds of the fixed head cols.
    fixed_head_cols_bounds: Bounds<Pixels>,
    /// The bounds of the right fixed head cols.
    fixed_right_head_cols_bounds: Bounds<Pixels>,

    col_groups: Vec<ColGroup>,
    /// The column indices of the visible columns, in display order. A
    /// display position indexes this.
    visible_cols: Vec<usize>,
    /// The display position of each column index, `None` when hidden.
    col_positions: Vec<Option<usize>>,
    header_layout: Vec<Vec<HeaderCell>>,

    /// Whether the table can loop selection, default is true.
    ///
    /// When the prev/next selection is out of the table bounds, the selection will loop to the other side.
    pub loop_selection: bool,
    /// Whether the table can select column.
    pub col_selectable: bool,
    /// Whether the table can select row.
    pub row_selectable: bool,
    /// Whether the table can select cell, default is false.
    ///
    /// When enabled:
    /// - Users can click on individual cells to select them
    /// - A row header column appears on the left for selecting entire rows
    ///   (can be hidden via [`Self::row_header`])
    /// - Keyboard navigation works at the cell level (arrow keys move between cells)
    /// - Right-click and double-click events are supported for cells
    pub cell_selectable: bool,
    /// Whether the row header column is visible when `cell_selectable` is enabled,
    /// default is `true`.
    ///
    /// Set to `false` to hide the narrow leftmost header column while keeping cell
    /// selection — useful when you want to put your own content (e.g. a row index
    /// column) on the left. When hidden, clicking the already-selected cell again
    /// escalates the selection to the whole row so users can still pick rows; row
    /// escalation requires `row_selectable` to be enabled.
    pub row_header: bool,
    /// Whether the table can sort.
    pub sortable: bool,
    /// Whether an unsorted-but-sortable column still draws a faint
    /// "chevrons up-down" icon inviting a click. Defaults to `true`
    /// (existing behavior, unchanged). Set to `false` to match a common
    /// alternative convention (e.g. AG Grid's own `unSortIcon: false`
    /// default): no icon at all until a column becomes the active sort,
    /// at which point the real ascending/descending arrow appears. Either
    /// way, clicking the column header still sorts it -- this only
    /// changes whether the invitation is drawn before that first click.
    pub unsorted_icon: bool,
    /// Whether the table can resize columns.
    pub col_resizable: bool,
    /// Whether the table can move columns.
    pub col_movable: bool,
    /// Enable/disable fixed columns feature.
    pub col_fixed: bool,

    pub vertical_scroll_handle: UniformListScrollHandle,
    pub horizontal_scroll_handle: VirtualListScrollHandle,

    selected_row: Option<usize>,
    selection_mode: SelectionMode,
    right_clicked_row: Option<usize>,
    right_clicked_cell: Option<(usize, usize)>,
    /// The column whose header cell has been right-clicked, taken by the
    /// header context menu.
    right_clicked_header: Option<usize>,
    selected_col: Option<usize>,
    selected_cell: Option<(usize, usize)>,

    /// The column index that is being resized.
    resizing_col: Option<usize>,

    /// The insertion gap while dragging a column header, as `(gap, to_ix)`:
    /// `gap` is a display position (`0..=visible_cols_count`), the dragged
    /// column shows between the visible columns `gap - 1` and `gap` on drop,
    /// by moving it to the column index `to_ix`.
    col_drag_gap: Option<(usize, usize)>,

    /// The visible range of the rows and columns.
    visible_range: TableVisibleRange,

    _measure: Vec<Duration>,
    _load_more_task: Task<()>,
}

impl<D> TableState<D>
where
    D: TableDelegate,
{
    /// Create a new TableState with the given delegate.
    pub fn new(delegate: D, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            options: TableOptions::default(),
            delegate,
            col_groups: Vec::new(),
            visible_cols: Vec::new(),
            col_positions: Vec::new(),
            header_layout: Vec::new(),
            horizontal_scroll_handle: VirtualListScrollHandle::new(),
            vertical_scroll_handle: UniformListScrollHandle::new(),
            selection_mode: SelectionMode::Row,
            selected_row: None,
            right_clicked_row: None,
            right_clicked_cell: None,
            right_clicked_header: None,
            selected_col: None,
            selected_cell: None,
            resizing_col: None,
            col_drag_gap: None,
            bounds: Bounds::default(),
            fixed_head_cols_bounds: Bounds::default(),
            fixed_right_head_cols_bounds: Bounds::default(),
            visible_range: TableVisibleRange::default(),
            loop_selection: true,
            col_selectable: true,
            row_selectable: true,
            cell_selectable: false,
            row_header: true,
            sortable: true,
            unsorted_icon: true,
            col_movable: true,
            col_resizable: true,
            col_fixed: true,
            _load_more_task: Task::ready(()),
            _measure: Vec::new(),
        };

        this.prepare_col_groups(cx);
        this
    }

    /// Returns a reference to the delegate.
    pub fn delegate(&self) -> &D {
        &self.delegate
    }

    /// Returns a mutable reference to the delegate.
    pub fn delegate_mut(&mut self) -> &mut D {
        &mut self.delegate
    }

    /// Set to loop selection, default to true.
    pub fn loop_selection(mut self, loop_selection: bool) -> Self {
        self.loop_selection = loop_selection;
        self
    }

    /// Set to enable/disable column movable, default to true.
    pub fn col_movable(mut self, col_movable: bool) -> Self {
        self.col_movable = col_movable;
        self
    }

    /// Set to enable/disable column resizable, default to true.
    pub fn col_resizable(mut self, col_resizable: bool) -> Self {
        self.col_resizable = col_resizable;
        self
    }

    /// Set to enable/disable column sortable, default true
    pub fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// Set to `false` to draw no icon at all on a sortable-but-not-currently-
    /// sorted column header, showing the real ascending/descending arrow only
    /// once that column becomes the active sort. Default `true` (existing
    /// behavior: a faint "chevrons up-down" invites the click).
    pub fn unsorted_icon(mut self, unsorted_icon: bool) -> Self {
        self.unsorted_icon = unsorted_icon;
        self
    }

    /// Set to enable/disable row selectable, default true
    pub fn row_selectable(mut self, row_selectable: bool) -> Self {
        self.row_selectable = row_selectable;
        self
    }

    /// Set to enable/disable column selectable, default true
    pub fn col_selectable(mut self, col_selectable: bool) -> Self {
        self.col_selectable = col_selectable;
        self
    }

    /// Set to enable/disable cell selection, default is false.
    ///
    /// When enabled:
    /// - Individual cells become selectable by clicking
    /// - A row header column appears on the left side (can be hidden via [`Self::row_header`])
    /// - Keyboard navigation operates at the cell level
    /// - Cell-specific events (SelectCell, DoubleClickedCell, RightClickedCell) are emitted
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let table_state = cx.new(|cx| {
    ///     TableState::new(delegate, cx)
    ///         .cell_selectable(true)  // Enable cell selection
    ///         .row_selectable(true)   // Also allow row selection via row header
    /// });
    /// ```
    pub fn cell_selectable(mut self, cell_selectable: bool) -> Self {
        self.cell_selectable = cell_selectable;
        self
    }

    /// Set whether the row header column is shown, default is `true`.
    ///
    /// Only effective when `cell_selectable` is `true` — otherwise the row header
    /// column is never rendered. Hide it when you want to use the leftmost column
    /// for your own content (e.g. a row index column).
    ///
    /// When hidden, the first click on a cell selects the cell; clicking the
    /// already-selected cell again escalates to selecting the whole row, so users
    /// can still pick rows without the dedicated header column. The row escalation
    /// requires `row_selectable` to be enabled.
    pub fn row_header(mut self, row_header: bool) -> Self {
        self.row_header = row_header;
        self
    }

    /// When we update columns or rows, we need to refresh the table.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.prepare_col_groups(cx);
    }

    /// Scroll to the row at the given index.
    pub fn scroll_to_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        self.vertical_scroll_handle
            .scroll_to_item(row_ix, ScrollStrategy::Top);
        cx.notify();
    }

    // Scroll to the column at the given index.
    pub fn scroll_to_col(&mut self, col_ix: usize, cx: &mut Context<Self>) {
        // A hidden column has nowhere to scroll to, and a right fixed column
        // is always visible.
        let Some(pos) = self.col_position(col_ix) else {
            cx.notify();
            return;
        };
        if pos >= self.fixed_right_start() {
            cx.notify();
            return;
        }

        let item_ix = pos.saturating_sub(self.fixed_left_cols_count());

        self.horizontal_scroll_handle
            .scroll_to_item(item_ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Returns whether the column at `col_ix` is visible, false for an
    /// index out of range.
    ///
    /// See [`Column::hidden`](Column#structfield.hidden).
    pub fn column_visible(&self, col_ix: usize) -> bool {
        self.col_position(col_ix).is_some()
    }

    /// Returns the column indices of the visible columns, in display order.
    pub fn visible_columns(&self) -> &[usize] {
        &self.visible_cols
    }

    /// Show or hide the column at `col_ix`.
    ///
    /// A hidden column keeps its index, so the delegate keeps all of its
    /// columns and none is renumbered. The delegate is told by
    /// [`TableDelegate::column_visibility_changed`]; note that
    /// [`TableState::refresh`] reads [`Column::hidden`](Column#structfield.hidden)
    /// from the delegate again.
    pub fn set_column_visible(&mut self, col_ix: usize, visible: bool, cx: &mut Context<Self>) {
        let Some(col_group) = self.col_groups.get_mut(col_ix) else {
            return;
        };
        if col_group.column.hidden != visible {
            return;
        }

        col_group.column.hidden = !visible;
        self.col_drag_gap = None;
        self.update_header_layout(cx);
        self.delegate.column_visibility_changed(col_ix, visible, cx);
        cx.notify();
    }

    /// Move the column at `col_ix` so that it ends up at the index `to_ix`,
    /// as dropping a dragged column header does, and returns whether it was
    /// moved.
    ///
    /// A move is refused, and nothing changes, when an index is out of
    /// range, when the column would leave its fixed region (see
    /// [`ColumnFixed`]) or when [`TableDelegate::can_move_column`] refuses
    /// it. Unlike a header drag, it does not check [`Self::col_movable`] or
    /// [`Column::movable`](Column#structfield.movable), and it can move a
    /// hidden column.
    ///
    /// An accepted move calls [`TableDelegate::move_column`], reorders the
    /// table's columns and emits [`TableEvent::MoveColumn`].
    pub fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.can_move_column(col_ix, to_ix, cx) {
            return false;
        }

        self.delegate.move_column(col_ix, to_ix, window, cx);
        let col_group = self.col_groups.remove(col_ix);
        self.col_groups.insert(to_ix, col_group);
        self.update_header_layout(cx);

        cx.emit(TableEvent::MoveColumn(col_ix, to_ix));
        cx.notify();
        true
    }

    /// Whether moving the column at `col_ix` to `to_ix` is a change that
    /// keeps the fixed regions and that the delegate accepts.
    fn can_move_column(&self, col_ix: usize, to_ix: usize, cx: &App) -> bool {
        let len = self.col_groups.len();
        if col_ix == to_ix || col_ix >= len || to_ix >= len {
            return false;
        }

        let ranks = self.region_ranks();
        move_keeps_regions(&ranks, col_ix, to_ix)
            && self.delegate.can_move_column(col_ix, to_ix, cx)
    }

    /// The fixed region rank of every column, see [`region_rank`].
    fn region_ranks(&self) -> Vec<u8> {
        self.col_groups
            .iter()
            .map(|g| region_rank(g.column.fixed, self.col_fixed))
            .collect()
    }

    /// The display position of the column at `col_ix`, `None` when it is
    /// hidden or out of range.
    fn col_position(&self, col_ix: usize) -> Option<usize> {
        self.col_positions.get(col_ix).copied().flatten()
    }

    /// Whether the column at `col_ix` is shown in the right fixed region.
    fn is_fixed_right(&self, col_ix: usize) -> bool {
        self.col_position(col_ix)
            .is_some_and(|pos| pos >= self.fixed_right_start())
    }

    /// Returns the selected row index.
    pub fn selected_row(&self) -> Option<usize> {
        self.selected_row
    }

    /// Sets the selected row to the given index.
    pub fn set_selected_row(&mut self, row_ix: usize, cx: &mut Context<Self>) {
        let is_down = match self.selected_row {
            Some(selected_row) => row_ix > selected_row,
            None => true,
        };

        cx.stop_propagation();
        self.selection_mode = SelectionMode::Row;
        self.right_clicked_row = None;
        self.selected_row = Some(row_ix);
        if let Some(row_ix) = self.selected_row {
            self.vertical_scroll_handle.scroll_to_item(
                row_ix,
                if is_down {
                    ScrollStrategy::Bottom
                } else {
                    ScrollStrategy::Top
                },
            );
        }
        cx.emit(TableEvent::SelectRow(row_ix));
        cx.emit(TableEvent::RightClickedRow(None));
        cx.notify();
    }

    /// Returns the row that has been right clicked.
    pub fn right_clicked_row(&self) -> Option<usize> {
        self.right_clicked_row
    }

    /// Set or clear the right-clicked row state.
    ///
    /// Pass `None` to clear — useful when opening a header context menu
    /// to prevent the row context menu from appearing simultaneously.
    pub fn set_right_clicked_row(&mut self, row: Option<usize>, cx: &mut Context<Self>) {
        self.right_clicked_row = row;
        cx.notify();
    }

    /// Returns the selected column index.
    pub fn selected_col(&self) -> Option<usize> {
        self.selected_col
    }

    /// Sets the selected col to the given index.
    pub fn set_selected_col(&mut self, col_ix: usize, cx: &mut Context<Self>) {
        self.selection_mode = SelectionMode::Column;
        self.selected_col = Some(col_ix);
        if let Some(col_ix) = self.selected_col {
            self.scroll_to_col(col_ix, cx);
        }
        cx.emit(TableEvent::SelectColumn(col_ix));
        cx.notify();
    }

    /// Returns the selected cell as `(row_ix, col_ix)`.
    ///
    /// Returns `None` if no cell is currently selected or if the table is in row/column selection mode.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// if let Some((row_ix, col_ix)) = table_state.read(cx).selected_cell() {
    ///     println!("Selected cell: ({}, {})", row_ix, col_ix);
    /// }
    /// ```
    pub fn selected_cell(&self) -> Option<(usize, usize)> {
        self.selected_cell
    }

    /// Sets the selected cell to the given row and column indices.
    ///
    /// This method:
    /// - Switches the table to cell selection mode
    /// - Scrolls to make the cell visible (centered vertically)
    /// - Emits a [`TableEvent::SelectCell`] event
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Select the cell at row 5, column 3
    /// table_state.update(cx, |state, cx| {
    ///     state.set_selected_cell(5, 3, cx);
    /// });
    /// ```
    pub fn set_selected_cell(&mut self, row_ix: usize, col_ix: usize, cx: &mut Context<Self>) {
        self.selection_mode = SelectionMode::Cell;
        self.selected_cell = Some((row_ix, col_ix));

        // `Nearest`, not `Center`: gpui's own `scroll_to_item` already skips
        // scrolling a fully visible row (`scroll_strict: false` gates the
        // whole match on `is_above || is_below`), so `Center` only ever
        // fired for a row *partly* clipped at an edge -- and recentring the
        // whole table for a partial clip is still wrong, just for a
        // narrower case than "every click" first suggested. `Nearest` does
        // the minimal reveal that case actually wants: nothing when the row
        // is visible, no more than needed when it is not. Also fixes
        // keyboard navigation, which shares this method and does not need
        // its own opt-out.
        self.vertical_scroll_handle
            .scroll_to_item(row_ix, ScrollStrategy::Nearest);
        self.scroll_to_col(col_ix, cx);

        cx.emit(TableEvent::SelectCell(row_ix, col_ix));
        cx.notify();
    }

    /// Clear the selection of the table.
    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selection_mode = SelectionMode::Row;
        self.selected_row = None;
        self.selected_col = None;
        self.selected_cell = None;
        cx.emit(TableEvent::ClearSelection);
        cx.notify();
    }

    /// Returns the visible range of the rows and columns.
    ///
    /// See [`TableVisibleRange`].
    pub fn visible_range(&self) -> &TableVisibleRange {
        &self.visible_range
    }

    /// Dump table data.
    ///
    /// Returns a tuple of (headers, rows) where each row is a vector of cell values.
    pub fn dump(&self, cx: &App) -> (Vec<String>, Vec<Vec<String>>) {
        // Get header row
        let columns_count = self.delegate.columns_count(cx);
        let mut headers = Vec::with_capacity(columns_count);
        for col_ix in 0..columns_count {
            let column = self.delegate.column(col_ix, cx);
            headers.push(column.name.to_string());
        }

        // Get data rows
        let rows_count = self.delegate.rows_count(cx);
        let mut rows = Vec::with_capacity(rows_count);
        for row_ix in 0..rows_count {
            let mut row = Vec::with_capacity(columns_count);
            for col_ix in 0..columns_count {
                row.push(self.delegate.cell_text(row_ix, col_ix, cx));
            }
            rows.push(row);
        }

        (headers, rows)
    }

    /// Re-compute the header layout from the current delegate.
    ///
    /// Call this after changing delegate state that affects `group_headers`.
    pub fn refresh_header_layout(&mut self, cx: &mut Context<Self>) {
        self.update_header_layout(cx);
        cx.notify();
    }

    fn prepare_col_groups(&mut self, cx: &mut Context<Self>) {
        self.col_groups = (0..self.delegate.columns_count(cx))
            .map(|col_ix| {
                let column = self.delegate().column(col_ix, cx);
                ColGroup {
                    width: column.width,
                    bounds: Bounds::default(),
                    column,
                }
            })
            .collect();

        self.update_header_layout(cx);
    }

    /// Recompute the visible columns, and the header layout from them.
    fn update_header_layout(&mut self, cx: &mut Context<Self>) {
        self.visible_cols = visible_col_indices(self.col_groups.iter().map(|g| g.column.hidden));
        self.col_positions = vec![None; self.col_groups.len()];
        for (pos, &col_ix) in self.visible_cols.iter().enumerate() {
            self.col_positions[col_ix] = Some(pos);
        }

        let group_rows = self.delegate.group_headers(cx);

        let mut layout = match group_rows.as_ref() {
            Some(rows) => Vec::with_capacity(rows.len() + 1),
            None => Vec::with_capacity(1),
        };

        if let Some(group_rows) = group_rows {
            for row in group_rows {
                let mut cell_row = Vec::with_capacity(row.len());
                let mut current_leaf_ix = 0;
                for group in row {
                    // A group spans column indices, it shows over the visible
                    // ones only, and not at all when they are all hidden.
                    let mut width = px(0.);
                    let mut start_leaf_col_ix = None;
                    for col_ix in current_leaf_ix..current_leaf_ix + group.span {
                        if let Some(pos) = self.col_position(col_ix) {
                            width += self.col_groups[col_ix].width;
                            start_leaf_col_ix.get_or_insert(pos);
                        }
                    }
                    current_leaf_ix += group.span;
                    let Some(start_leaf_col_ix) = start_leaf_col_ix else {
                        continue;
                    };
                    cell_row.push(HeaderCell {
                        label: group.label.clone(),
                        width,
                        col_span: group.span,
                        is_leaf: false,
                        leaf_col_ix: None,
                        start_leaf_col_ix,
                    });
                }
                layout.push(cell_row);
            }
        }

        // `leaf_col_ix` is the column index, `start_leaf_col_ix` the display
        // position.
        let mut leaf_row = Vec::with_capacity(self.visible_cols.len());
        for (pos, &ix) in self.visible_cols.iter().enumerate() {
            let group = &self.col_groups[ix];
            leaf_row.push(HeaderCell {
                label: group.column.name.clone(),
                width: group.width,
                col_span: 1,
                is_leaf: true,
                leaf_col_ix: Some(ix),
                start_leaf_col_ix: pos,
            });
        }
        layout.push(leaf_row);

        self.header_layout = layout;
    }

    /// The number of visible left fixed columns, which take the first
    /// display positions.
    fn fixed_left_cols_count(&self) -> usize {
        self.visible_fixed_cols_count(ColumnFixed::Left)
    }

    fn fixed_right_cols_count(&self) -> usize {
        self.visible_fixed_cols_count(ColumnFixed::Right)
    }

    fn visible_fixed_cols_count(&self, fixed: ColumnFixed) -> usize {
        if !self.col_fixed {
            return 0;
        }

        self.visible_cols
            .iter()
            .filter(|&&ix| self.col_groups[ix].column.fixed == Some(fixed))
            .count()
    }

    /// The display position of the first right fixed column, which is also
    /// the end of the scrollable columns. Equals the visible columns count
    /// without right fixed columns.
    fn fixed_right_start(&self) -> usize {
        fixed_right_start(self.visible_cols.len(), self.fixed_right_cols_count())
    }

    /// The column index of the first visible column, 0 when none is.
    fn first_visible_col(&self) -> usize {
        self.visible_cols.first().copied().unwrap_or(0)
    }

    /// The column index of the last visible column, 0 when none is.
    fn last_visible_col(&self) -> usize {
        self.visible_cols.last().copied().unwrap_or(0)
    }

    /// The column index of the visible column before (or after, when
    /// `forward`) the column at `col_ix`, see [`step_position`].
    fn step_visible_col(&self, col_ix: usize, forward: bool) -> usize {
        let Some(pos) = self.col_position(col_ix) else {
            return self.first_visible_col();
        };
        let pos = step_position(pos, self.visible_cols.len(), forward, self.loop_selection);
        self.visible_cols[pos]
    }

    fn page_item_count(&self) -> usize {
        let row_height = self.options.size.table_row_height();
        let height = self.bounds.size.height;
        let count = (height / row_height).floor() as usize;
        count.saturating_sub(1).max(1)
    }

    fn on_row_right_click(
        &mut self,
        _: &MouseDownEvent,
        row_ix: Option<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.right_clicked_row = row_ix;
        self.right_clicked_cell = None;
        self.right_clicked_header = None;
        cx.emit(TableEvent::RightClickedRow(row_ix));
    }

    fn on_cell_right_click(
        &mut self,
        _: &MouseDownEvent,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.cell_selectable {
            return;
        }

        cx.stop_propagation();
        self.right_clicked_cell = Some((row_ix, col_ix));
        self.right_clicked_row = None;
        cx.emit(TableEvent::RightClickedCell(row_ix, col_ix));
    }

    fn on_row_left_click(
        &mut self,
        e: &ClickEvent,
        row_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.row_selectable {
            return;
        }

        self.set_selected_row(row_ix, cx);

        if e.click_count() == 2 {
            cx.emit(TableEvent::DoubleClickedRow(row_ix));
        }
    }

    fn on_col_head_click(&mut self, col_ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        if !self.col_selectable {
            return;
        }

        let Some(col_group) = self.col_groups.get(col_ix) else {
            return;
        };

        if !col_group.column.selectable {
            return;
        }

        self.set_selected_col(col_ix, cx)
    }

    fn on_cell_click(
        &mut self,
        e: &ClickEvent,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.cell_selectable {
            return;
        }

        cx.stop_propagation();

        let is_double_click = e.click_count() == 2;

        // When the row header column is hidden, a single click on the
        // already-selected cell escalates the selection to the entire row —
        // giving users a way to pick rows without the dedicated header column.
        // Double-clicks are passed through to `DoubleClickedCell` and never
        // trigger the escalation.
        let is_reselect =
            self.selection_mode.is_cell() && self.selected_cell == Some((row_ix, col_ix));
        let should_escalate_to_row =
            !self.row_header && self.row_selectable && is_reselect && !is_double_click;
        if should_escalate_to_row {
            self.set_selected_row(row_ix, cx);
            return;
        }

        self.set_selected_cell(row_ix, col_ix, cx);

        if is_double_click {
            cx.emit(TableEvent::DoubleClickedCell(row_ix, col_ix));
        }
    }

    fn has_selection(&self) -> bool {
        self.selected_row.is_some() || self.selected_col.is_some() || self.selected_cell.is_some()
    }

    pub(super) fn action_cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.has_selection() {
            self.clear_selection(cx);
            return;
        }
        cx.propagate();
    }

    pub(super) fn action_select_prev(
        &mut self,
        _: &SelectUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows_count = self.delegate.rows_count(cx);
        if rows_count < 1 {
            return;
        }

        // Cell selection mode: move up within the same column
        if self.selection_mode.is_cell() {
            if let Some((row_ix, col_ix)) = self.selected_cell {
                let new_row = if row_ix > 0 {
                    row_ix.saturating_sub(1)
                } else if self.loop_selection {
                    rows_count.saturating_sub(1)
                } else {
                    row_ix
                };
                self.set_selected_cell(new_row, col_ix, cx);
            } else {
                // No cell selected, select first cell
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Row selection mode
        let mut selected_row = self.selected_row.unwrap_or(0);
        if selected_row > 0 {
            selected_row = selected_row.saturating_sub(1);
        } else {
            if self.loop_selection {
                selected_row = rows_count.saturating_sub(1);
            }
        }

        self.set_selected_row(selected_row, cx);
    }

    pub(super) fn action_select_next(
        &mut self,
        _: &SelectDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows_count = self.delegate.rows_count(cx);
        if rows_count < 1 {
            return;
        }

        // Cell selection mode: move down within the same column
        if self.selection_mode.is_cell() {
            if let Some((row_ix, col_ix)) = self.selected_cell {
                let new_row = if row_ix < rows_count.saturating_sub(1) {
                    row_ix + 1
                } else if self.loop_selection {
                    0
                } else {
                    row_ix
                };
                self.set_selected_cell(new_row, col_ix, cx);
            } else {
                // No cell selected, select first cell
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Row selection mode
        let selected_row = match self.selected_row {
            Some(selected_row) if selected_row < rows_count.saturating_sub(1) => selected_row + 1,
            Some(selected_row) => {
                if self.loop_selection {
                    0
                } else {
                    selected_row
                }
            }
            _ => 0,
        };

        self.set_selected_row(selected_row, cx);
    }

    pub(super) fn action_select_first_column(
        &mut self,
        _: &SelectFirst,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cell selection mode: move to first cell in current row
        if self.selection_mode.is_cell() {
            if let Some((row_ix, _)) = self.selected_cell {
                self.set_selected_cell(row_ix, self.first_visible_col(), cx);
            } else {
                // No cell selected, select first cell of first row
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Column selection mode
        self.set_selected_col(self.first_visible_col(), cx);
    }

    pub(super) fn action_select_last_column(
        &mut self,
        _: &SelectLast,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let last_col = self.last_visible_col();

        // Cell selection mode: move to last cell in current row
        if self.selection_mode.is_cell() {
            if let Some((row_ix, _)) = self.selected_cell {
                self.set_selected_cell(row_ix, last_col, cx);
            } else {
                // No cell selected, select last cell of first row
                self.set_selected_cell(0, last_col, cx);
            }
            return;
        }

        // Column selection mode
        self.set_selected_col(last_col, cx);
    }

    pub(super) fn action_select_page_up(
        &mut self,
        _: &SelectPageUp,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let step = self.page_item_count();

        // Cell selection mode: move up by page within the same column
        if self.selection_mode.is_cell() {
            if let Some((row_ix, col_ix)) = self.selected_cell {
                let target = row_ix.saturating_sub(step);
                self.set_selected_cell(target, col_ix, cx);
            } else {
                // No cell selected, select first cell
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Row selection mode
        let current = self.selected_row.unwrap_or(0);
        let target = current.saturating_sub(step);
        self.set_selected_row(target, cx);
    }

    pub(super) fn action_select_page_down(
        &mut self,
        _: &SelectPageDown,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rows_count = self.delegate.rows_count(cx);
        if rows_count == 0 {
            return;
        }

        let step = self.page_item_count();

        // Cell selection mode: move down by page within the same column
        if self.selection_mode.is_cell() {
            if let Some((row_ix, col_ix)) = self.selected_cell {
                let max_row = rows_count.saturating_sub(1);
                let target = (row_ix + step).min(max_row);
                self.set_selected_cell(target, col_ix, cx);
            } else {
                // No cell selected, select first cell
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Row selection mode
        let current = self.selected_row.unwrap_or(0);
        let max_row = rows_count.saturating_sub(1);
        let target = (current + step).min(max_row);
        self.set_selected_row(target, cx);
    }

    pub(super) fn action_select_prev_col(
        &mut self,
        _: &SelectPrevColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cell selection mode: move left within the same row
        if self.selection_mode.is_cell() {
            if let Some((row_ix, col_ix)) = self.selected_cell {
                let new_col = self.step_visible_col(col_ix, false);
                self.set_selected_cell(row_ix, new_col, cx);
            } else {
                // No cell selected, select first cell
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Column selection mode
        let selected_col = self.step_visible_col(self.selected_col.unwrap_or(0), false);
        self.set_selected_col(selected_col, cx);
    }

    pub(super) fn action_select_next_col(
        &mut self,
        _: &SelectNextColumn,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cell selection mode: move right within the same row
        if self.selection_mode.is_cell() {
            if let Some((row_ix, col_ix)) = self.selected_cell {
                let new_col = self.step_visible_col(col_ix, true);
                self.set_selected_cell(row_ix, new_col, cx);
            } else {
                // No cell selected, select first cell
                self.set_selected_cell(0, self.first_visible_col(), cx);
            }
            return;
        }

        // Column selection mode
        let selected_col = self.step_visible_col(self.selected_col.unwrap_or(0), true);
        self.set_selected_col(selected_col, cx);
    }

    /// Scroll table when mouse position is near the edge of the table bounds.
    fn scroll_table_by_col_resizing(
        &mut self,
        mouse_position: Point<Pixels>,
        col_group: &ColGroup,
    ) {
        // Do nothing if pos out of the table bounds right for avoid scroll to the right.
        if mouse_position.x > self.bounds.right() {
            return;
        }

        let mut offset = self.horizontal_scroll_handle.offset();
        let col_bounds = col_group.bounds;

        if mouse_position.x < self.bounds.left()
            && col_bounds.right() < self.bounds.left() + px(20.)
        {
            offset.x += px(1.);
        } else if mouse_position.x > self.bounds.right()
            && col_bounds.right() > self.bounds.right() - px(20.)
        {
            offset.x -= px(1.);
        }

        self.horizontal_scroll_handle.set_offset(offset);
    }

    /// The `ix`` is the index of the col to resize,
    /// and the `size` is the new size for the col.
    fn resize_cols(&mut self, ix: usize, size: Pixels, _: &mut Window, cx: &mut Context<Self>) {
        if !self.col_resizable {
            return;
        }

        let mut changed = false;
        if let Some(col_group) = self.col_groups.get_mut(ix) {
            if col_group.is_resizable() {
                let new_width = size.clamp(col_group.column.min_width, col_group.column.max_width);
                if col_group.width != new_width {
                    col_group.width = new_width;
                    changed = true;
                }
            }
        }

        if changed {
            self.update_header_layout(cx);
            cx.notify();
        }
    }

    fn perform_sort(&mut self, col_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if !self.sortable {
            return;
        }

        let sort = self.col_groups.get(col_ix).and_then(|g| g.column.sort);
        if sort.is_none() {
            return;
        }

        let sort = sort.unwrap();
        let sort = match sort {
            ColumnSort::Ascending => ColumnSort::Default,
            ColumnSort::Descending => ColumnSort::Ascending,
            ColumnSort::Default => ColumnSort::Descending,
        };

        self.apply_sort(col_ix, sort, window, cx);
    }

    /// Sort by the column at `col_ix`, as clicking its header until it shows
    /// `sort` would. Does nothing when the table or the column is not sortable.
    pub fn sort_column(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.sortable || self.column_sort(col_ix).is_none() {
            return;
        }
        self.apply_sort(col_ix, sort, window, cx);
    }

    /// The sort the column at `col_ix` shows, `None` when it is not sortable.
    pub fn column_sort(&self, col_ix: usize) -> Option<ColumnSort> {
        self.col_groups.get(col_ix).and_then(|g| g.column.sort)
    }

    fn apply_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for (ix, col_group) in self.col_groups.iter_mut().enumerate() {
            if ix == col_ix {
                col_group.column.sort = Some(sort);
            } else {
                if col_group.column.sort.is_some() {
                    col_group.column.sort = Some(ColumnSort::Default);
                }
            }
        }

        self.delegate_mut().perform_sort(col_ix, sort, window, cx);

        cx.notify();
    }

    /// Resolve the insertion gap for a column-header drag at the window
    /// coordinate `x`, as `(gap, to_ix)` (see `col_drag_gap`), or `None` when
    /// dropping there would not move the dragged column at `drag_col_ix`, or
    /// the move is refused (see [`Self::can_move_column`]).
    fn drag_gap_at(&self, x: Pixels, drag_col_ix: usize, cx: &App) -> Option<(usize, usize)> {
        let drag_pos = self.col_position(drag_col_ix)?;
        let fixed_count = self.fixed_left_cols_count();
        let right_start = self.fixed_right_start();
        let cols_count = self.visible_cols.len();
        let has_right = right_start < cols_count;

        // Columns scrolled beneath the fixed region keep stale bounds, so
        // resolve `x` against the fixed columns alone when it falls in that
        // region, and against the visible scrollable columns otherwise.
        let in_left = fixed_count > 0 && x < self.fixed_head_cols_bounds.right();
        let in_right = has_right && x >= self.fixed_right_head_cols_bounds.left();
        let candidates = if in_left {
            0..fixed_count
        } else if in_right {
            right_start..cols_count
        } else {
            self.calculate_visible_leaf_col_range(fixed_count).0
        };

        // Fixed columns are taken by position, so left and right fixed
        // columns only move among themselves, and no other column moves
        // among them.
        if (drag_pos < fixed_count) != in_left {
            return None;
        }
        if has_right && (drag_pos >= right_start) != in_right {
            return None;
        }

        // The gap sits after the last candidate column whose center is left of `x`.
        let mut gap = candidates.start;
        for pos in candidates {
            if x < self.col_groups[self.visible_cols[pos]].bounds.center().x {
                break;
            }
            gap = pos + 1;
        }

        // No gap if dropping there would put the dragged column back to
        // where it already is.
        if gap == drag_pos || gap == drag_pos + 1 {
            return None;
        }

        let to_ix = gap_move_index(&self.visible_cols, &self.region_ranks(), drag_col_ix, gap)?;
        if !self.delegate.can_move_column(drag_col_ix, to_ix, cx) {
            return None;
        }
        Some((gap, to_ix))
    }

    /// Dispatch delegate's `load_more` method when the visible range is near the end.
    fn load_more_if_need(
        &mut self,
        rows_count: usize,
        visible_end: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let threshold = self.delegate.load_more_threshold();
        // Securely handle subtract logic to prevent attempt to subtract with overflow
        if visible_end >= rows_count.saturating_sub(threshold) {
            if !self.delegate.has_more(cx) {
                return;
            }

            self._load_more_task = cx.spawn_in(window, async move |view, window| {
                _ = view.update_in(window, |view, window, cx| {
                    view.delegate.load_more(window, cx);
                });
            });
        }
    }

    fn update_visible_range_if_need(
        &mut self,
        visible_range: Range<usize>,
        axis: Axis,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Skip when visible range is only 1 item.
        // The visual_list will use first item to measure.
        if visible_range.len() <= 1 {
            return;
        }

        if axis == Axis::Vertical {
            if self.visible_range.rows == visible_range {
                return;
            }
            self.delegate_mut()
                .visible_rows_changed(visible_range.clone(), window, cx);
            self.visible_range.rows = visible_range;
        } else {
            if self.visible_range.cols == visible_range {
                return;
            }
            self.delegate_mut()
                .visible_columns_changed(visible_range.clone(), window, cx);
            self.visible_range.cols = visible_range;
        }
    }

    fn render_cell(
        &self,
        _row_ix: Option<usize>,
        col_ix: usize,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Div {
        let Some(col_group) = self.col_groups.get(col_ix) else {
            return div();
        };

        let col_width = col_group.width;
        let col_padding = col_group.column.paddings;

        div()
            .w(col_width)
            .h_full()
            .flex_shrink_0()
            .overflow_hidden()
            .whitespace_nowrap()
            .table_cell_size(self.options.size)
            .map(|this| match col_padding {
                Some(padding) => this
                    .pl(padding.left)
                    .pr(padding.right)
                    .pt(padding.top)
                    .pb(padding.bottom),
                None => this,
            })
    }

    /// Show Column selection style, when the column is selected and the selection state is Column.
    /// Note: When a cell is selected, column selection style is not shown.
    fn render_col_wrap(
        &self,
        _row_ix: Option<usize>,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let el = h_flex().h_full();
        let selectable = self.col_selectable
            && self
                .col_groups
                .get(col_ix)
                .map(|col_group| col_group.column.selectable)
                .unwrap_or(false);

        // Don't show column selection if a cell is selected
        if self.selection_mode.is_cell() {
            return el;
        }

        if selectable && self.selected_col == Some(col_ix) && self.selection_mode.is_column() {
            el.bg(cx.theme().tokens.table_active)
        } else {
            el
        }
    }

    fn render_resize_handle(
        &self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        const HANDLE_SIZE: Pixels = px(2.);

        let resizable = self.col_resizable
            && self
                .col_groups
                .get(ix)
                .map(|col| col.is_resizable())
                .unwrap_or(false);
        if !resizable {
            return div().into_any_element();
        }

        let group_id = SharedString::from(format!("resizable-handle:{}", ix));
        // A right fixed column grows to the left, so its handle sits on its
        // left edge, laid over the cell (see `render_th`).
        let on_left_edge = self.is_fixed_right(ix);

        h_flex()
            .id(("resizable-handle", ix))
            .group(group_id.clone())
            .occlude()
            .cursor_col_resize()
            .h_full()
            .w(HANDLE_SIZE)
            .map(|this| {
                if on_left_edge {
                    this.absolute().top_0().left_0().justify_start()
                } else {
                    this.ml(-(HANDLE_SIZE)).justify_end()
                }
            })
            .items_center()
            .child(
                div()
                    .h_full()
                    .justify_center()
                    .bg(cx.theme().table_row_border)
                    .group_hover(&group_id, |this| this.bg(cx.theme().border).h_full())
                    .w(px(1.)),
            )
            .on_drag_move(
                cx.listener(move |view, e: &DragMoveEvent<ResizeColumn>, window, cx| {
                    match e.drag(cx) {
                        ResizeColumn((entity_id, ix)) => {
                            if cx.entity_id() != *entity_id {
                                return;
                            }

                            // sync col widths into real widths
                            // TODO: Consider to remove this, this may not need now.
                            // for (_, col_group) in view.col_groups.iter_mut().enumerate() {
                            //     col_group.width = col_group.bounds.size.width;
                            // }

                            let ix = *ix;
                            view.resizing_col = Some(ix);

                            let col_group = view
                                .col_groups
                                .get(ix)
                                .expect("BUG: invalid col index")
                                .clone();

                            if view.is_fixed_right(ix) {
                                // The right edge of a right fixed column
                                // stays put while it resizes, so measure
                                // from it; the region does not scroll.
                                view.resize_cols(
                                    ix,
                                    col_group.bounds.right() - e.event.position.x,
                                    window,
                                    cx,
                                );
                            } else {
                                view.resize_cols(
                                    ix,
                                    e.event.position.x - HANDLE_SIZE - col_group.bounds.left(),
                                    window,
                                    cx,
                                );

                                // scroll the table if the drag is near the edge
                                view.scroll_table_by_col_resizing(e.event.position, &col_group);
                            }
                        }
                    };
                }),
            )
            .on_drag(ResizeColumn((cx.entity_id(), ix)), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| {
                    if view.resizing_col.is_none() {
                        return;
                    }

                    view.resizing_col = None;

                    let new_widths = view.col_groups.iter().map(|g| g.width).collect();
                    cx.emit(TableEvent::ColumnWidthsChanged(new_widths));
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// Render the row header cell (when cell_selectable is enabled)
    fn render_row_header_cell(
        &self,
        row_ix: usize,
        is_head: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(("row-header", row_ix))
            .w_3()
            .h_full()
            .border_r_1()
            .border_color(cx.theme().table_row_border)
            .bg(cx.theme().tokens.table_head)
            .flex_shrink_0()
            .table_cell_size(self.options.size)
            .when(!is_head, |this| {
                this.when(self.row_selectable, |this| {
                    this.on_click(cx.listener(move |table, _, _window, cx| {
                        table.set_selected_row(row_ix, cx);
                    }))
                })
            })
    }

    fn render_sort_icon(
        &self,
        col_ix: usize,
        col_group: &ColGroup,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.sortable {
            return None;
        }

        let Some(sort) = col_group.column.sort else {
            return None;
        };

        let icon = match sort {
            ColumnSort::Ascending => IconName::SortAscending,
            ColumnSort::Descending => IconName::SortDescending,
            // Sortable, but not the active sort: draw the faint invitation
            // icon only if the caller still wants one (`unsorted_icon`,
            // default `true`, matches the behavior this arm always had
            // before that field existed). `unsorted_icon: false` skips this
            // arm entirely -- no icon until the column IS the active sort.
            ColumnSort::Default if !self.unsorted_icon => return None,
            ColumnSort::Default => IconName::ChevronsUpDown,
        };
        let is_on = !matches!(sort, ColumnSort::Default);

        Some(
            div()
                .id(("icon-sort", col_ix))
                .p(px(2.))
                .rounded(cx.theme().radius / 2.)
                .map(|this| match is_on {
                    true => this,
                    false => this.opacity(0.5),
                })
                .hover(|this| this.bg(cx.theme().tokens.secondary).opacity(7.))
                .active(|this| this.bg(cx.theme().tokens.secondary_active).opacity(1.))
                .on_click(
                    cx.listener(move |table, _, window, cx| table.perform_sort(col_ix, window, cx)),
                )
                .child(
                    Icon::new(icon)
                        .size_3()
                        .text_color(cx.theme().secondary_foreground),
                ),
        )
    }

    /// Render the column header.
    /// The children must be one by one items.
    /// Because the horizontal scroll handle will use the child_item_bounds to
    /// calculate the item position for itself's `scroll_to_item` method.
    fn render_th(&mut self, col_ix: usize, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let entity_id = cx.entity_id();
        let col_group = self.col_groups.get(col_ix).expect("BUG: invalid col index");

        let movable = self.col_movable && col_group.column.movable;
        let paddings = col_group.column.paddings;
        let name = col_group.column.name.clone();
        let is_fixed_right = self.is_fixed_right(col_ix);
        let pos = self
            .col_position(col_ix)
            .expect("BUG: render a hidden column");

        h_flex()
            .h_full()
            // For the absolutely positioned resize handle of a right fixed column.
            .when(is_fixed_right, |this| this.relative())
            .child(
                self.render_cell(None, col_ix, window, cx)
                    .id(("col-header", col_ix))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.on_col_head_click(col_ix, window, cx);
                        // Sorting a column was previously reachable only by
                        // clicking `render_sort_icon`'s own small icon --
                        // `unsorted_icon(false)` hides that icon entirely
                        // for an unsorted column, which removed the only
                        // way to ever START sorting one (an already-sorted
                        // column still had its real arrow to click). Both
                        // `perform_sort` and `on_col_head_click` self-gate
                        // (no-op on a non-sortable/non-selectable column),
                        // so calling both from the same click is safe
                        // regardless of `unsorted_icon`'s setting.
                        this.perform_sort(col_ix, window, cx);
                    }))
                    // For `TableDelegate::header_context_menu`.
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, _, cx| {
                            this.right_clicked_header = Some(col_ix);
                            this.right_clicked_row = None;
                            this.right_clicked_cell = None;
                            cx.notify();
                        }),
                    )
                    .child(
                        h_flex()
                            .size_full()
                            .justify_between()
                            .items_center()
                            .child(self.delegate.render_th(col_ix, window, cx))
                            .when_some(paddings, |this, paddings| {
                                // Leave right space for the sort icon, if this column have custom padding
                                let offset_pr =
                                    self.options.size.table_cell_padding().right - paddings.right;
                                this.pr(offset_pr.max(px(0.)))
                            })
                            .children(self.render_sort_icon(col_ix, &col_group, window, cx))
                            // After the sort icon, not before -- see
                            // `TableDelegate::render_th_trailing`'s own doc
                            // comment for why this exists as a separate hook
                            // rather than something a delegate puts inside
                            // its own `render_th`.
                            .child(self.delegate.render_th_trailing(col_ix, window, cx)),
                    )
                    .when(movable, |this| {
                        this.on_drag(
                            DragColumn {
                                entity_id,
                                col_ix,
                                name,
                                width: col_group.width,
                            },
                            |drag, _, _, cx| {
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            },
                        )
                    })
                    .map(|this| {
                        // Draw the insertion indicator on the left edge of the gap
                        // column, or on the right edge of the last column for the
                        // trailing gap. Use an absolutely positioned overlay instead
                        // of a border, to avoid shifting the cell content.
                        let last_gap = pos + 1 == self.visible_cols.len();
                        match self.col_drag_gap {
                            Some((gap, _))
                                if cx.has_active_drag()
                                    && (gap == pos || (last_gap && gap == pos + 1)) =>
                            {
                                let right_side = gap == pos + 1;
                                this.relative().child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .bottom_0()
                                        .w(px(2.))
                                        .map(|d| if right_side { d.right_0() } else { d.left_0() })
                                        .bg(cx.theme().drag_border),
                                )
                            }
                            _ => this,
                        }
                    }),
            )
            // resize handle
            .child(self.render_resize_handle(col_ix, window, cx))
            // to save the bounds of this col.
            .on_prepaint({
                let view = cx.entity().clone();
                move |bounds, _, cx| view.update(cx, |r, _| r.col_groups[col_ix].bounds = bounds)
            })
    }

    /// Compute the visible non-fixed leaf-column range for header rendering.
    ///
    /// Returns `(visible_range, left_spacer_width)` where:
    /// - `visible_range` is the column-index range that should be rendered.
    /// - `left_spacer_width` is the total width of the off-screen left columns,
    ///   used as a spacer div to keep visible columns at the correct position.
    ///
    /// On the first frame `self.bounds` is zero, so a fallback that covers all
    /// columns is returned to avoid a blank header on initial paint.
    fn calculate_visible_leaf_col_range(
        &self,
        left_columns_count: usize,
    ) -> (Range<usize>, Pixels) {
        // Right fixed columns are rendered in their own region, never in the
        // scrollable one.
        let scroll_end = self.fixed_right_start();

        if self.bounds.size.width == px(0.) {
            return (left_columns_count..scroll_end, px(0.));
        }

        let fixed_width = self.fixed_head_cols_bounds.size.width
            + if scroll_end < self.visible_cols.len() {
                self.fixed_right_head_cols_bounds.size.width
            } else {
                px(0.)
            };
        let available_width = (self.bounds.size.width - fixed_width).max(px(0.));
        // The scroll handle offset is negative when scrolled right; negate it
        // to obtain a positive distance from the left edge of the scroll area.
        let scroll_x = (-self.horizontal_scroll_handle.offset().x).max(px(0.));

        visible_leaf_col_range(
            |pos| self.col_groups[self.visible_cols[pos]].width,
            left_columns_count..scroll_end,
            scroll_x,
            available_width,
        )
    }

    fn render_table_header(
        &mut self,
        left_columns_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().clone();
        let horizontal_scroll_handle = self.horizontal_scroll_handle.clone();

        // Header leaf-column virtualization.
        //
        // `render_th` creates interactive elements with resize-handle listeners.
        // Calling it for every column every frame is O(n) in column count; with
        // 1000+ columns this alone drops FPS below 60 even in release mode.
        //
        // We restrict rendering to the columns currently visible inside the
        // overflow-scroll viewport, surrounding them with inert spacer divs:
        //
        //   [left_spacer] [visible columns…] [right_spacer] [last_empty_col]
        //
        // The spacers preserve the flex container's total content width so that
        // the scrollbar range stays correct.
        //
        // Right fixed columns (`scroll_end..total_cols`) render in their own
        // region after the scrollable one, pinned to the right edge.
        //
        // These ranges are display positions, hidden columns have none.
        let total_cols = self.visible_cols.len();
        let scroll_end = self.fixed_right_start();
        let (visible_col_range, left_spacer) =
            self.calculate_visible_leaf_col_range(left_columns_count);

        let layout_len = self.header_layout.len();

        // Reset fixed head columns bounds, if no fixed columns are present
        if left_columns_count == 0 {
            self.fixed_head_cols_bounds = Bounds::default();
        }
        if scroll_end == total_cols {
            self.fixed_right_head_cols_bounds = Bounds::default();
        }

        let mut header = self.delegate_mut().render_header(window, cx);
        let style = header.style().clone();
        let layout = self.header_layout.clone();

        header
            .h_flex()
            .w_full()
            .flex_shrink_0()
            .bg(cx.theme().tokens.table_head)
            .text_color(cx.theme().table_head_foreground)
            .refine_style(&style)
            .on_drag_move(cx.listener(|table, e: &DragMoveEvent<DragColumn>, _, cx| {
                let drag = e.drag(cx);
                let (drag_entity_id, drag_col_ix) = (drag.entity_id, drag.col_ix);

                let gap =
                    if drag_entity_id == cx.entity_id() && e.bounds.contains(&e.event.position) {
                        table.drag_gap_at(e.event.position.x, drag_col_ix, cx)
                    } else {
                        None
                    };

                if table.col_drag_gap != gap {
                    table.col_drag_gap = gap;
                    cx.notify();
                }
            }))
            .on_drop(cx.listener(|table, drag: &DragColumn, window, cx| {
                if drag.entity_id != cx.entity_id() {
                    return;
                }

                // Insert the dragged column into the indicated gap.
                let Some((_, to_ix)) = table.col_drag_gap.take() else {
                    return;
                };
                table.move_column(drag.col_ix, to_ix, window, cx);
            }))
            .when(self.cell_selectable && self.row_header, |this| {
                this.child(self.render_row_header_cell(0, true, cx))
            })
            .when(left_columns_count > 0, |this| {
                let view = view.clone();
                // Render left fixed columns
                this.child(
                    h_flex()
                        .relative()
                        .h_full()
                        .bg(cx.theme().tokens.table_head)
                        .child(v_flex().min_w_full().flex_shrink_0().children(
                            layout.iter().enumerate().map(|(_row_ix, row_cells)| {
                                h_flex()
                                    .min_w_full()
                                    .h(self.options.size.table_row_height())
                                    .border_b_1()
                                    .border_color(cx.theme().border)
                                    .children(row_cells.iter().filter_map(|cell| {
                                        if cell.start_leaf_col_ix < left_columns_count {
                                            if cell.is_leaf {
                                                if let Some(ix) = cell.leaf_col_ix {
                                                    return Some(
                                                        self.render_th(ix, window, cx)
                                                            .into_any_element(),
                                                    );
                                                }
                                            } else {
                                                return Some(
                                                    self.delegate_mut()
                                                        .render_group_th(
                                                            &cell.label,
                                                            cell.col_span,
                                                            cell.width,
                                                            window,
                                                            cx,
                                                        )
                                                        .into_any_element(),
                                                );
                                            }
                                        }
                                        None
                                    }))
                            }),
                        ))
                        .child(
                            // Fixed columns border
                            div()
                                .absolute()
                                .top_0()
                                .right_0()
                                .bottom_0()
                                .w_0()
                                .flex_shrink_0()
                                .border_r_1()
                                .border_color(cx.theme().border),
                        )
                        .on_prepaint(move |bounds, _, cx| {
                            view.update(cx, |r, _| r.fixed_head_cols_bounds = bounds)
                        }),
                )
            })
            .child(
                // Columns
                h_flex()
                    .id("table-head")
                    .size_full()
                    .overflow_scroll()
                    .relative()
                    .track_scroll(&horizontal_scroll_handle)
                    .bg(cx.theme().tokens.table_head)
                    .child(v_flex().min_w_full().flex_shrink_0().children(
                        layout.iter().enumerate().map(|(row_ix, row_cells)| {
                            let is_leaf_row = row_ix + 1 == layout_len;
                            h_flex()
                                .min_w_full()
                                .h(self.options.size.table_row_height())
                                .border_b_1()
                                .border_color(cx.theme().border)
                                .map(|this| {
                                    if is_leaf_row {
                                        // Leaf row: apply the spacer virtualization pattern.
                                        // Only columns in `visible_range` are rendered; the two
                                        // spacer divs preserve the container's total content width
                                        // so the scrollbar range stays correct.
                                        this.when(left_spacer > px(0.), |r| {
                                            r.child(div().w(left_spacer).h_full().flex_shrink_0())
                                        })
                                        .children(row_cells.iter().filter_map(|cell| {
                                            if cell.is_leaf {
                                                let ix = cell.leaf_col_ix?;
                                                if !visible_col_range
                                                    .contains(&cell.start_leaf_col_ix)
                                                {
                                                    return None;
                                                }
                                                Some(
                                                    self.render_th(ix, window, cx)
                                                        .into_any_element(),
                                                )
                                            } else {
                                                None
                                            }
                                        }))
                                        .when(visible_col_range.end < scroll_end, |r| {
                                            let right_spacer: Pixels = self.visible_cols
                                                [visible_col_range.end..scroll_end]
                                                .iter()
                                                .map(|&ix| self.col_groups[ix].width)
                                                .sum();
                                            r.child(div().w(right_spacer).h_full().flex_shrink_0())
                                        })
                                        .child(self.delegate.render_last_empty_col(window, cx))
                                    } else {
                                        // Group header rows have far fewer cells (one per group),
                                        // so the cost of rendering all of them is negligible.
                                        this.children(row_cells.iter().filter_map(|cell| {
                                            if cell.start_leaf_col_ix >= left_columns_count
                                                && cell.start_leaf_col_ix < scroll_end
                                            {
                                                if cell.is_leaf {
                                                    if let Some(ix) = cell.leaf_col_ix {
                                                        return Some(
                                                            self.render_th(ix, window, cx)
                                                                .into_any_element(),
                                                        );
                                                    }
                                                } else {
                                                    return Some(
                                                        self.delegate_mut()
                                                            .render_group_th(
                                                                &cell.label,
                                                                cell.col_span,
                                                                cell.width,
                                                                window,
                                                                cx,
                                                            )
                                                            .into_any_element(),
                                                    );
                                                }
                                            }
                                            None
                                        }))
                                        .child(self.delegate.render_last_empty_col(window, cx))
                                    }
                                })
                        }),
                    )),
            )
            .when(scroll_end < total_cols, |this| {
                let view = view.clone();
                // Render right fixed columns
                this.child(
                    h_flex()
                        .relative()
                        .h_full()
                        .flex_shrink_0()
                        .bg(cx.theme().tokens.table_head)
                        .child(
                            v_flex()
                                .flex_shrink_0()
                                .children(layout.iter().map(|row_cells| {
                                    h_flex()
                                        .min_w_full()
                                        .h(self.options.size.table_row_height())
                                        .border_b_1()
                                        .border_color(cx.theme().border)
                                        .children(row_cells.iter().filter_map(|cell| {
                                            if cell.start_leaf_col_ix < scroll_end {
                                                return None;
                                            }
                                            if cell.is_leaf {
                                                let ix = cell.leaf_col_ix?;
                                                Some(
                                                    self.render_th(ix, window, cx)
                                                        .into_any_element(),
                                                )
                                            } else {
                                                Some(
                                                    self.delegate_mut()
                                                        .render_group_th(
                                                            &cell.label,
                                                            cell.col_span,
                                                            cell.width,
                                                            window,
                                                            cx,
                                                        )
                                                        .into_any_element(),
                                                )
                                            }
                                        }))
                                })),
                        )
                        .child(
                            // Fixed columns border
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .bottom_0()
                                .w_0()
                                .flex_shrink_0()
                                .border_l_1()
                                .border_color(cx.theme().border),
                        )
                        .on_prepaint(move |bounds, _, cx| {
                            view.update(cx, |r, _| r.fixed_right_head_cols_bounds = bounds)
                        }),
                )
            })
    }

    /// Render the right fixed cells of a row, the display positions
    /// `right_start..`.
    fn render_fixed_right_cells(
        &mut self,
        row_ix: usize,
        right_start: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let cols = self.visible_cols[right_start..].to_vec();
        h_flex()
            .relative()
            .h_full()
            .flex_shrink_0()
            .children(
                cols.into_iter()
                    .map(|col_ix| self.render_body_cell(row_ix, col_ix, window, cx))
                    .collect::<Vec<_>>(),
            )
            .child(
                // Fixed columns border
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .bottom_0()
                    .w_0()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(cx.theme().border),
            )
    }

    /// Render a body cell of a fixed column, with its selection overlays and
    /// cell click handlers.
    fn render_body_cell(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let is_cell_selected =
            self.selected_cell == Some((row_ix, col_ix)) && self.selection_mode.is_cell();
        let is_cell_right_clicked = self.right_clicked_cell == Some((row_ix, col_ix));

        self.render_col_wrap(Some(row_ix), col_ix, window, cx)
            .child(
                self.render_cell(Some(row_ix), col_ix, window, cx)
                    .id(format!("table-cell:{}:{}", row_ix, col_ix))
                    .relative()
                    .child(self.measure_render_td(row_ix, col_ix, window, cx))
                    .when(is_cell_selected, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .bg(cx.theme().tokens.table_active)
                                .border_1()
                                .border_color(cx.theme().table_active_border),
                        )
                    })
                    .when(is_cell_right_clicked && !is_cell_selected, |this| {
                        this.child(
                            div()
                                .absolute()
                                .inset_0()
                                .border_1()
                                .border_color(cx.theme().table_active_border.opacity(0.5)),
                        )
                    })
                    .when(self.cell_selectable, |this| {
                        this.on_click(cx.listener(move |table, e, window, cx| {
                            table.on_cell_click(e, row_ix, col_ix, window, cx);
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |table, e, window, cx| {
                                table.on_cell_right_click(e, row_ix, col_ix, window, cx);
                            }),
                        )
                    }),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table_row(
        &mut self,
        row_ix: usize,
        rows_count: usize,
        left_columns_count: usize,
        col_sizes: Rc<Vec<gpui::Size<Pixels>>>,
        is_filled: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let horizontal_scroll_handle = self.horizontal_scroll_handle.clone();
        let is_stripe_row = self.options.stripe && row_ix % 2 != 0;
        let is_selected = self.selected_row == Some(row_ix);
        let view = cx.entity().clone();
        let row_height = self.options.size.table_row_height();
        let right_start = self.fixed_right_start();

        if row_ix < rows_count {
            let is_last_row = row_ix + 1 == rows_count;
            let need_render_border = is_selected || !is_last_row || !is_filled;

            let mut tr = self.delegate.render_tr(row_ix, window, cx);
            let style = tr.style().clone();

            tr.h_flex()
                .w_full()
                .h(row_height)
                .when(need_render_border, |this| {
                    this.border_b_1().border_color(cx.theme().table_row_border)
                })
                .when(is_stripe_row, |this| this.bg(cx.theme().tokens.table_even))
                .refine_style(&style)
                .hover(|this| {
                    if is_selected || self.right_clicked_row == Some(row_ix) {
                        this
                    } else {
                        this.bg(cx.theme().tokens.table_hover)
                    }
                })
                .when(self.cell_selectable && self.row_header, |this| {
                    this.child(self.render_row_header_cell(row_ix, false, cx))
                })
                .when(left_columns_count > 0, |this| {
                    // Left fixed columns
                    this.child(
                        h_flex()
                            .relative()
                            .h_full()
                            .children({
                                let mut items = Vec::with_capacity(left_columns_count);

                                (0..left_columns_count).for_each(|pos| {
                                    let col_ix = self.visible_cols[pos];
                                    let is_cell_selected = self.selected_cell
                                        == Some((row_ix, col_ix))
                                        && self.selection_mode.is_cell();
                                    let is_cell_right_clicked =
                                        self.right_clicked_cell == Some((row_ix, col_ix));

                                    items.push(
                                        self.render_col_wrap(Some(row_ix), col_ix, window, cx)
                                            .child(
                                                self.render_cell(Some(row_ix), col_ix, window, cx)
                                                    .id(format!("table-cell:{}:{}", row_ix, col_ix))
                                                    .relative()
                                                    .child(self.measure_render_td(
                                                        row_ix, col_ix, window, cx,
                                                    ))
                                                    .when(is_cell_selected, |this| {
                                                        this.child(
                                                            div()
                                                                .absolute()
                                                                .inset_0()
                                                                .bg(cx.theme().tokens.table_active)
                                                                .border_1()
                                                                .border_color(
                                                                    cx.theme().table_active_border,
                                                                ),
                                                        )
                                                    })
                                                    .when(
                                                        is_cell_right_clicked && !is_cell_selected,
                                                        |this| {
                                                            this.child(
                                                                div()
                                                                    .absolute()
                                                                    .inset_0()
                                                                    .border_1()
                                                                    .border_color(
                                                                        cx.theme()
                                                                            .table_active_border
                                                                            .opacity(0.5),
                                                                    ),
                                                            )
                                                        },
                                                    )
                                                    .when(self.cell_selectable, |this| {
                                                        this.on_click(cx.listener(
                                                            move |table, e, window, cx| {
                                                                table.on_cell_click(
                                                                    e, row_ix, col_ix, window, cx,
                                                                );
                                                            },
                                                        ))
                                                        .on_mouse_down(
                                                            MouseButton::Right,
                                                            cx.listener(
                                                                move |table, e, window, cx| {
                                                                    table.on_cell_right_click(
                                                                        e, row_ix, col_ix, window,
                                                                        cx,
                                                                    );
                                                                },
                                                            ),
                                                        )
                                                    }),
                                            ),
                                    );
                                });

                                items
                            })
                            .child(
                                // Fixed columns border
                                div()
                                    .absolute()
                                    .top_0()
                                    .right_0()
                                    .bottom_0()
                                    .w_0()
                                    .flex_shrink_0()
                                    .border_r_1()
                                    .border_color(cx.theme().border),
                            ),
                    )
                })
                .child(
                    h_flex()
                        .flex_1()
                        .h_full()
                        .overflow_hidden()
                        .relative()
                        .child(
                            crate::virtual_list::virtual_list(
                                view,
                                row_ix,
                                Axis::Horizontal,
                                col_sizes,
                                {
                                    move |table, visible_range: Range<usize>, window, cx| {
                                        table.update_visible_range_if_need(
                                            visible_range.clone(),
                                            Axis::Horizontal,
                                            window,
                                            cx,
                                        );

                                        let mut items = Vec::with_capacity(
                                            visible_range.end - visible_range.start,
                                        );

                                        visible_range.for_each(|item_ix| {
                                            let col_ix =
                                                table.visible_cols[item_ix + left_columns_count];
                                            let is_cell_selected = table.selected_cell
                                                == Some((row_ix, col_ix))
                                                && table.selection_mode.is_cell();
                                            let is_cell_right_clicked =
                                                table.right_clicked_cell == Some((row_ix, col_ix));

                                            let el = table
                                                .render_col_wrap(Some(row_ix), col_ix, window, cx)
                                                .child(
                                                    table
                                                        .render_cell(
                                                            Some(row_ix),
                                                            col_ix,
                                                            window,
                                                            cx,
                                                        )
                                                        .id(format!(
                                                            "table-cell-{}:{}",
                                                            row_ix, col_ix
                                                        ))
                                                        .relative()
                                                        .child(table.measure_render_td(
                                                            row_ix, col_ix, window, cx,
                                                        ))
                                                        .when(is_cell_selected, |this| {
                                                            this.child(
                                                                div()
                                                                    .absolute()
                                                                    .inset_0()
                                                                    .bg(cx
                                                                        .theme()
                                                                        .tokens
                                                                        .table_active)
                                                                    .border_1()
                                                                    .border_color(
                                                                        cx.theme()
                                                                            .table_active_border,
                                                                    ),
                                                            )
                                                        })
                                                        .when(
                                                            is_cell_right_clicked
                                                                && !is_cell_selected,
                                                            |this| {
                                                                this.child(
                                                                    div()
                                                                        .absolute()
                                                                        .inset_0()
                                                                        .border_1()
                                                                        .border_color(
                                                                            cx.theme()
                                                                                .table_active_border
                                                                                .opacity(0.5),
                                                                        ),
                                                                )
                                                            },
                                                        )
                                                        .when(table.cell_selectable, |this| {
                                                            this.on_click(cx.listener(
                                                                move |table, e, window, cx| {
                                                                    cx.stop_propagation();
                                                                    table.on_cell_click(
                                                                        e, row_ix, col_ix, window,
                                                                        cx,
                                                                    );
                                                                },
                                                            ))
                                                            .on_mouse_down(
                                                                MouseButton::Right,
                                                                cx.listener(
                                                                    move |table, e, window, cx| {
                                                                        table.on_cell_right_click(
                                                                            e, row_ix, col_ix,
                                                                            window, cx,
                                                                        );
                                                                    },
                                                                ),
                                                            )
                                                        }),
                                                );

                                            items.push(el);
                                        });

                                        items
                                    }
                                },
                            )
                            .with_scroll_handle(&self.horizontal_scroll_handle),
                        )
                        .child(self.delegate.render_last_empty_col(window, cx)),
                )
                .when(right_start < self.visible_cols.len(), |this| {
                    // Right fixed columns
                    this.child(self.render_fixed_right_cells(row_ix, right_start, window, cx))
                })
                // Row selected style
                // Note: Don't show row selection if a cell is selected
                .when_some(self.selected_row, |this, _| {
                    this.when(is_selected && self.selection_mode.is_row(), |this| {
                        this.map(|this| {
                            if cx.theme().list.active_highlight {
                                this.border_color(gpui::transparent_white()).child(
                                    div()
                                        .top(if row_ix == 0 { px(0.) } else { px(-1.) })
                                        .left(px(0.))
                                        .right(px(0.))
                                        .bottom(px(-1.))
                                        .absolute()
                                        .bg(cx.theme().tokens.table_active)
                                        .border_1()
                                        .border_color(cx.theme().table_active_border),
                                )
                            } else {
                                this.bg(cx.theme().tokens.accent)
                            }
                        })
                    })
                })
                // Row right click row style
                .when(self.right_clicked_row == Some(row_ix), |this| {
                    this.border_color(gpui::transparent_white()).child(
                        div()
                            .top(if row_ix == 0 { px(0.) } else { px(-1.) })
                            .left(px(0.))
                            .right(px(0.))
                            .bottom(px(-1.))
                            .absolute()
                            .border_1()
                            .border_color(cx.theme().selection),
                    )
                })
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, e, window, cx| {
                        this.on_row_right_click(e, Some(row_ix), window, cx);
                    }),
                )
                .on_click(cx.listener(move |this, e, window, cx| {
                    this.on_row_left_click(e, row_ix, window, cx);
                }))
        } else {
            // Render fake rows to fill the rest table space
            self.delegate
                .render_tr(row_ix, window, cx)
                .h_flex()
                .w_full()
                .h(row_height)
                .border_b_1()
                .border_color(cx.theme().table_row_border)
                .when(is_stripe_row, |this| this.bg(cx.theme().tokens.table_even))
                .when(self.cell_selectable && self.row_header, |this| {
                    // Render empty row header cell for fake rows
                    this.child(
                        div()
                            .w(px(40.))
                            .h_full()
                            .flex_shrink_0()
                            .table_cell_size(self.options.size),
                    )
                })
                .children(self.visible_cols.clone().into_iter().map(|col_ix| {
                    h_flex()
                        .left(horizontal_scroll_handle.offset().x)
                        .child(self.render_cell(None, col_ix, window, cx))
                }))
                .child(self.delegate.render_last_empty_col(window, cx))
        }
    }

    /// Calculate the extra rows needed to fill the table empty space when `stripe` is true.
    fn calculate_extra_rows_needed(
        &self,
        total_height: Pixels,
        actual_height: Pixels,
        row_height: Pixels,
    ) -> usize {
        let mut extra_rows_needed = 0;

        let remaining_height = total_height - actual_height;
        if remaining_height > px(0.) {
            extra_rows_needed = (remaining_height / row_height).floor() as usize;
        }

        extra_rows_needed
    }

    #[inline]
    fn measure_render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !crate::measure_enable() {
            return self
                .delegate
                .render_td(row_ix, col_ix, window, cx)
                .into_any_element();
        }

        let start = std::time::Instant::now();
        let el = self.delegate.render_td(row_ix, col_ix, window, cx);
        self._measure.push(start.elapsed());
        el.into_any_element()
    }

    fn measure(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if !crate::measure_enable() {
            return;
        }

        // Print avg measure time of each td
        if self._measure.len() > 0 {
            let total = self
                ._measure
                .iter()
                .fold(Duration::default(), |acc, d| acc + *d);
            let avg = total / self._measure.len() as u32;
            eprintln!(
                "last render {} cells total: {:?}, avg: {:?}",
                self._measure.len(),
                total,
                avg,
            );
        }
        self._measure.clear();
    }

    fn render_vertical_scrollbar(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        Some(
            div()
                .absolute()
                .top(self.options.size.table_row_height() * self.header_layout.len().max(1) as f32)
                .right_0()
                .bottom_0()
                .w(Scrollbar::width())
                .child(
                    Scrollbar::vertical(&self.vertical_scroll_handle)
                        .viewport_from_layout()
                        .max_fps(60),
                ),
        )
    }

    fn render_horizontal_scrollbar(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .left(self.fixed_head_cols_bounds.size.width)
            .right(self.fixed_right_head_cols_bounds.size.width)
            .bottom_0()
            .h(Scrollbar::width())
            .child(Scrollbar::horizontal(&self.horizontal_scroll_handle).viewport_from_layout())
    }
}

impl<D> Focusable for TableState<D>
where
    D: TableDelegate,
{
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
impl<D> EventEmitter<TableEvent> for TableState<D> where D: TableDelegate {}

impl<D> Render for TableState<D>
where
    D: TableDelegate,
{
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.measure(window, cx);

        let left_columns_count = self.fixed_left_cols_count();
        // The scrollable columns are the display positions
        // `left_columns_count..right_start`.
        let right_start = self.fixed_right_start();
        let rows_count = self.delegate.rows_count(cx);
        let loading = self.delegate.loading(cx);

        let row_height = self.options.size.table_row_height();
        let total_height = self
            .vertical_scroll_handle
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height;
        let actual_height = row_height * rows_count as f32;
        let extra_rows_count =
            self.calculate_extra_rows_needed(total_height, actual_height, row_height);
        let render_rows_count = if self.options.stripe {
            rows_count + extra_rows_count
        } else {
            rows_count
        };
        let right_clicked_row = self.right_clicked_row;
        let is_filled = total_height > Pixels::ZERO && total_height <= actual_height;

        let loading_view = if loading {
            Some(
                self.delegate
                    .render_loading(self.options.size, window, cx)
                    .into_any_element(),
            )
        } else {
            None
        };

        let empty_view = if rows_count == 0 {
            Some(
                div()
                    .size_full()
                    .child(self.delegate.render_empty(window, cx))
                    .into_any_element(),
            )
        } else {
            None
        };

        let inner_table = v_flex()
            .id("table-inner")
            .size_full()
            .overflow_hidden()
            .child(self.render_table_header(left_columns_count, window, cx))
            .context_menu({
                let view = cx.entity().clone();
                move |this, window: &mut Window, cx: &mut Context<PopupMenu>| {
                    // A right-clicked header cell takes its menu from
                    // `header_context_menu`, once.
                    let header_col_ix =
                        view.update(cx, |table, _| table.right_clicked_header.take());
                    if let Some(col_ix) = header_col_ix {
                        view.update(cx, |table, cx| {
                            table
                                .delegate_mut()
                                .header_context_menu(col_ix, this, window, cx)
                        })
                    } else if let Some(row_ix) = view.read(cx).right_clicked_row {
                        view.update(cx, |menu, cx| {
                            menu.delegate_mut().context_menu(row_ix, this, window, cx)
                        })
                    } else {
                        this
                    }
                }
            })
            .map(|this| {
                if rows_count == 0 {
                    this.children(empty_view)
                } else {
                    this.child(
                        h_flex().id("table-body").flex_grow_1().size_full().child(
                            uniform_list(
                                "table-uniform-list",
                                render_rows_count,
                                cx.processor(
                                    move |table, visible_range: Range<usize>, window, cx| {
                                        // Use `col.width` (always up-to-date) rather than
                                        // `col.bounds.size.width`, which is only set after
                                        // prepaint and is therefore zero on the first frame.
                                        let col_sizes: Rc<Vec<gpui::Size<Pixels>>> = Rc::new(
                                            table.visible_cols[left_columns_count..right_start]
                                                .iter()
                                                .map(|&ix| gpui::Size {
                                                    width: table.col_groups[ix].width,
                                                    height: px(0.),
                                                })
                                                .collect(),
                                        );

                                        table.load_more_if_need(
                                            rows_count,
                                            visible_range.end,
                                            window,
                                            cx,
                                        );
                                        table.update_visible_range_if_need(
                                            visible_range.clone(),
                                            Axis::Vertical,
                                            window,
                                            cx,
                                        );

                                        if visible_range.end > rows_count {
                                            table.scroll_to_row(
                                                std::cmp::min(
                                                    visible_range.start,
                                                    rows_count.saturating_sub(1),
                                                ),
                                                cx,
                                            );
                                        }

                                        let mut items = Vec::with_capacity(
                                            visible_range.end.saturating_sub(visible_range.start),
                                        );

                                        // Render fake rows to fill the table
                                        visible_range.for_each(|row_ix| {
                                            // Render real rows for available data
                                            items.push(table.render_table_row(
                                                row_ix,
                                                rows_count,
                                                left_columns_count,
                                                col_sizes.clone(),
                                                is_filled,
                                                window,
                                                cx,
                                            ));
                                        });

                                        items
                                    },
                                ),
                            )
                            .flex_grow_1()
                            .size_full()
                            .with_sizing_behavior(ListSizingBehavior::Auto)
                            .track_scroll(&self.vertical_scroll_handle)
                            .into_any_element(),
                        ),
                    )
                }
            });

        div()
            .size_full()
            .children(loading_view)
            .when(!loading, |this| {
                this.child(inner_table)
                    .child(ScrollableMask::new(
                        Axis::Horizontal,
                        &self.horizontal_scroll_handle,
                    ))
                    // Keep vertical wheel scrolling from leaking into an
                    // ancestor scroller. Skipped when the table is empty:
                    // the `uniform_list` is not rendered then, so the
                    // handle's offset and `max_offset` are stale.
                    .when(rows_count > 0, |this| {
                        this.child(ScrollableMask::new(
                            Axis::Vertical,
                            &self.vertical_scroll_handle.0.borrow().base_handle,
                        ))
                    })
                    .when(right_clicked_row.is_some(), |this| {
                        this.on_mouse_down_out(cx.listener(|this, e, window, cx| {
                            this.on_row_right_click(e, None, window, cx);
                            cx.notify();
                        }))
                    })
            })
            .on_prepaint({
                let state = cx.entity();
                move |bounds, _, cx| state.update(cx, |state, _| state.bounds = bounds)
            })
            .when(!window.is_inspector_picking(cx), |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .size_full()
                        .when(self.options.scrollbar_visible.bottom, |this| {
                            this.child(self.render_horizontal_scrollbar(window, cx))
                        })
                        .when(
                            self.options.scrollbar_visible.right && rows_count > 0,
                            |this| this.children(self.render_vertical_scrollbar(window, cx)),
                        ),
                )
            })
    }
}

/// The index of the first right fixed column of `cols_count` columns whose
/// last `right_count` columns are fixed on the right.
fn fixed_right_start(cols_count: usize, right_count: usize) -> usize {
    cols_count.saturating_sub(right_count)
}

/// The column indices of the columns that are not hidden, in order.
fn visible_col_indices(hidden: impl Iterator<Item = bool>) -> Vec<usize> {
    hidden
        .enumerate()
        .filter_map(|(ix, hidden)| (!hidden).then_some(ix))
        .collect()
}

/// The display position after (or before, when not `forward`) `pos` among
/// `count` positions, wrapping around when `wrap`, else staying at the end.
fn step_position(pos: usize, count: usize, forward: bool, wrap: bool) -> usize {
    let last = count.saturating_sub(1);
    match (forward, pos) {
        (true, pos) if pos < last => pos + 1,
        (true, _) if wrap => 0,
        (false, pos) if pos > 0 => pos - 1,
        (false, _) if wrap => last,
        (_, pos) => pos,
    }
}

/// The order of a column's fixed region: left fixed columns come first,
/// then the scrollable ones, then the right fixed ones. Without `col_fixed`
/// every column is scrollable.
fn region_rank(fixed: Option<ColumnFixed>, col_fixed: bool) -> u8 {
    match fixed {
        _ if !col_fixed => 1,
        Some(ColumnFixed::Left) => 0,
        None => 1,
        Some(ColumnFixed::Right) => 2,
    }
}

/// Whether moving the column `from` to the index `to` keeps it inside its
/// fixed region, given the [`region_rank`] of every column: its new
/// neighbours must not rank after it on its left, nor before it on its right.
fn move_keeps_regions(ranks: &[u8], from: usize, to: usize) -> bool {
    let rank = ranks[from];
    // The ranks once `from` is removed, where `to` is the insertion index.
    let rest = |ix: usize| ranks[if ix < from { ix } else { ix + 1 }];
    let prev_ok = to == 0 || rest(to - 1) <= rank;
    let next_ok = to + 1 >= ranks.len() || rank <= rest(to);
    prev_ok && next_ok
}

/// The column index to move the column `from` to, so that it shows in the
/// display gap `gap` (between the visible columns `gap - 1` and `gap`), or
/// `None` when no such index keeps the fixed regions, see
/// [`move_keeps_regions`].
///
/// With hidden columns around the gap, several indices show the same way;
/// the first one that keeps the regions is taken.
fn gap_move_index(visible: &[usize], ranks: &[u8], from: usize, gap: usize) -> Option<usize> {
    if ranks.len() < 2 || gap > visible.len() {
        return None;
    }

    // Insertion indices count the columns once `from` is removed.
    let removed = |ix: usize| if ix > from { ix - 1 } else { ix };
    let start = match gap {
        0 => 0,
        gap => removed(visible[gap - 1]) + 1,
    };
    let end = match visible.get(gap) {
        Some(&ix) => removed(ix),
        None => ranks.len() - 1,
    };

    (start..=end).find(|&to| to != from && move_keeps_regions(ranks, from, to))
}

/// Compute the visible range of the scrollable columns in `cols`.
///
/// Returns `(visible_range, left_spacer_width)`, see
/// `TableState::calculate_visible_leaf_col_range`.
fn visible_leaf_col_range(
    width: impl Fn(usize) -> Pixels,
    cols: Range<usize>,
    scroll_x: Pixels,
    available_width: Pixels,
) -> (Range<usize>, Pixels) {
    // Walk left-to-right through the scrollable columns to find the first one
    // whose right edge enters the viewport. The accumulated width of the
    // skipped columns becomes the left spacer width.
    let mut range_start = cols.start;
    let mut left_spacer = px(0.);
    let mut cumulative = px(0.);
    for i in cols.clone() {
        let right_edge = cumulative + width(i);
        if right_edge > scroll_x {
            range_start = i;
            left_spacer = cumulative;
            break;
        }
        cumulative = right_edge;
    }

    // Continue from `range_start` (skipping already-scanned columns) to
    // find the last column still within the viewport. The 200 px overdraw
    // buffer prevents a visible flash when the user scrolls quickly.
    let right_bound = scroll_x + available_width + px(200.);
    let mut range_end = cols.end;
    let mut cumulative = left_spacer; // already summed widths before `range_start`
    for i in range_start..cols.end {
        cumulative += width(i);
        if cumulative > right_bound {
            range_end = (i + 1).min(cols.end);
            break;
        }
    }

    (range_start..range_end, left_spacer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_right_start() {
        assert_eq!(fixed_right_start(5, 0), 5);
        assert_eq!(fixed_right_start(5, 2), 3);
        assert_eq!(fixed_right_start(1, 3), 0);
    }

    #[test]
    fn test_visible_leaf_col_range_excludes_fixed_columns() {
        // 2 left fixed, 6 scrollable, 2 right fixed columns of 100px.
        let width = |_| px(100.);

        // Everything fits: all the scrollable columns, none of the fixed ones.
        let (range, spacer) = visible_leaf_col_range(width, 2..8, px(0.), px(1000.));
        assert_eq!(range, 2..8);
        assert_eq!(spacer, px(0.));

        // Scrolled by 250px: the 3rd scrollable column (ix 4) is the first
        // visible one, and the overdraw ends before the right fixed columns.
        let (range, spacer) = visible_leaf_col_range(width, 2..8, px(250.), px(100.));
        assert_eq!(range, 4..8);
        assert_eq!(spacer, px(200.));

        // Narrow viewport: the range ends after the overdraw buffer.
        let (range, _) = visible_leaf_col_range(width, 2..8, px(0.), px(50.));
        assert_eq!(range, 2..5);
    }

    #[test]
    fn test_visible_col_indices() {
        let hidden = [false, true, false, false, true];
        assert_eq!(visible_col_indices(hidden.into_iter()), vec![0, 2, 3]);
        assert_eq!(
            visible_col_indices([true, true].into_iter()),
            Vec::<usize>::new()
        );
        assert_eq!(visible_col_indices(std::iter::empty()), Vec::<usize>::new());
    }

    #[test]
    fn test_step_position() {
        assert_eq!(step_position(1, 3, true, false), 2);
        assert_eq!(step_position(2, 3, true, false), 2);
        assert_eq!(step_position(2, 3, true, true), 0);
        assert_eq!(step_position(1, 3, false, false), 0);
        assert_eq!(step_position(0, 3, false, false), 0);
        assert_eq!(step_position(0, 3, false, true), 2);
        assert_eq!(step_position(0, 0, true, true), 0);
        assert_eq!(step_position(0, 0, false, true), 0);
    }

    #[test]
    fn test_region_rank() {
        assert_eq!(region_rank(Some(ColumnFixed::Left), true), 0);
        assert_eq!(region_rank(None, true), 1);
        assert_eq!(region_rank(Some(ColumnFixed::Right), true), 2);
        assert_eq!(region_rank(Some(ColumnFixed::Left), false), 1);
        assert_eq!(region_rank(Some(ColumnFixed::Right), false), 1);
    }

    #[test]
    fn test_move_keeps_regions() {
        // L L S S S R
        let ranks = [0, 0, 1, 1, 1, 2];

        // Within each region, up to the region borders.
        assert!(move_keeps_regions(&ranks, 0, 1));
        assert!(move_keeps_regions(&ranks, 2, 4));
        assert!(move_keeps_regions(&ranks, 4, 2));

        // A scrollable column may not go into the left or right region.
        assert!(!move_keeps_regions(&ranks, 3, 0));
        assert!(!move_keeps_regions(&ranks, 3, 1));
        assert!(!move_keeps_regions(&ranks, 2, 5));

        // A left column may not leave the left region, a right one the right.
        assert!(!move_keeps_regions(&ranks, 1, 2));
        assert!(!move_keeps_regions(&ranks, 0, 4));
        assert!(!move_keeps_regions(&ranks, 5, 4));

        // Without fixed columns everything goes.
        let ranks = [1, 1, 1];
        assert!(move_keeps_regions(&ranks, 0, 2));
        assert!(move_keeps_regions(&ranks, 2, 0));
    }

    #[test]
    fn test_gap_move_index_matches_plain_reorder() {
        // No hidden columns, no fixed regions: the classic
        // `if from < gap { gap - 1 } else { gap }`.
        let visible = [0, 1, 2, 3, 4];
        let ranks = [1; 5];
        for from in 0..5 {
            for gap in 0..=5 {
                if gap == from || gap == from + 1 {
                    continue;
                }
                let expected = if from < gap { gap - 1 } else { gap };
                assert_eq!(
                    gap_move_index(&visible, &ranks, from, gap),
                    Some(expected),
                    "from {from} gap {gap}"
                );
            }
        }
    }

    #[test]
    fn test_gap_move_index_with_hidden_columns() {
        // Columns 0..6, 1 and 4 hidden, so the display is [0, 2, 3, 5].
        let visible = [0, 2, 3, 5];
        let ranks = [1; 6];

        // Column 5 to the first gap: before column 0.
        assert_eq!(gap_move_index(&visible, &ranks, 5, 0), Some(0));
        // Column 0 between 3 and 5: right after column 3.
        assert_eq!(gap_move_index(&visible, &ranks, 0, 3), Some(3));
        // Column 0 to the end: after column 5.
        assert_eq!(gap_move_index(&visible, &ranks, 0, 4), Some(5));
        // Column 5 between 0 and 2: right after column 0.
        assert_eq!(gap_move_index(&visible, &ranks, 5, 1), Some(1));
    }

    #[test]
    fn test_gap_move_index_keeps_fixed_regions() {
        // L L S S R, all visible.
        let visible = [0, 1, 2, 3, 4];
        let ranks = [0, 0, 1, 1, 2];

        // A scrollable column dropped inside the left region is refused; at
        // the left region's end it lands first among the scrollable ones.
        assert_eq!(gap_move_index(&visible, &ranks, 3, 0), None);
        assert_eq!(gap_move_index(&visible, &ranks, 3, 1), None);
        assert_eq!(gap_move_index(&visible, &ranks, 3, 2), Some(2));
        // Into the right region is refused, as is a right column out of it.
        assert_eq!(gap_move_index(&visible, &ranks, 2, 5), None);
        assert_eq!(gap_move_index(&visible, &ranks, 4, 2), None);
        // A left column stays among the left ones.
        assert_eq!(gap_move_index(&visible, &ranks, 0, 2), Some(1));
        assert_eq!(gap_move_index(&visible, &ranks, 0, 3), None);

        // The left column 1 is hidden: a scrollable column dropped right
        // after the visible left column lands after the hidden one too.
        let visible = [0, 2, 3, 4];
        assert_eq!(gap_move_index(&visible, &ranks, 3, 1), Some(2));
    }

    #[test]
    fn test_visible_leaf_col_range_without_scrollable_columns() {
        let (range, spacer) = visible_leaf_col_range(|_| px(100.), 3..3, px(0.), px(500.));
        assert_eq!(range, 3..3);
        assert_eq!(spacer, px(0.));
    }
}
