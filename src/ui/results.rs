use crate::{model::Column as DataColumn, pagination::Pagination, worker::Event};
use gpui_kit::base::actions::{
    Cancel, SelectDown, SelectFirst, SelectLast, SelectNextColumn, SelectPageDown, SelectPageUp,
    SelectPrevColumn, SelectUp,
};
use gpui_kit::component::{
    ActiveTheme, Sizable, WindowExt,
    input::{Copy, SelectAll},
    menu::{PopupMenu, PopupMenuItem},
    table::{Column, DataTable, TableDelegate, TableState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::{borrow::Cow, ops::RangeInclusive, time::Duration};

actions!(
    qrow_results,
    [
        ExtendSelectionUp,
        ExtendSelectionDown,
        ExtendSelectionLeft,
        ExtendSelectionRight
    ]
);

/// The key context around the results table.
const CONTEXT: &str = "Results";

/// Key bindings of the results table. The table binds the plain arrow keys;
/// the results view handles those actions before the table does.
pub(super) fn bindings() -> [KeyBinding; 6] {
    let context = Some("Results > DataTable");
    [
        KeyBinding::new("shift-up", ExtendSelectionUp, context),
        KeyBinding::new("shift-down", ExtendSelectionDown, context),
        KeyBinding::new("shift-left", ExtendSelectionLeft, context),
        KeyBinding::new("shift-right", ExtendSelectionRight, context),
        KeyBinding::new("cmd-c", Copy, context),
        KeyBinding::new("cmd-a", SelectAll, context),
    ]
}

/// A rectangle of result cells. Rows are indices into all downloaded rows.
/// Columns are indices into the result columns, without the row number column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The cell where the selection started. Shift extends from it.
    pub anchor: (usize, usize),
    /// The cell that moves with the keyboard, the pointer, or Shift-click.
    pub focus: (usize, usize),
}
impl Selection {
    pub fn cell(row: usize, column: usize) -> Self {
        Self {
            anchor: (row, column),
            focus: (row, column),
        }
    }
    pub fn rows(&self) -> RangeInclusive<usize> {
        self.anchor.0.min(self.focus.0)..=self.anchor.0.max(self.focus.0)
    }
    pub fn columns(&self) -> RangeInclusive<usize> {
        self.anchor.1.min(self.focus.1)..=self.anchor.1.max(self.focus.1)
    }
    pub fn contains(&self, row: usize, column: usize) -> bool {
        self.rows().contains(&row) && self.columns().contains(&column)
    }
    pub fn is_cell(&self) -> bool {
        self.anchor == self.focus
    }
}

pub struct Results {
    pub columns: Vec<DataColumn>,
    pub rows: crate::export::Rows,
    pub pagination: Pagination,
    pub export: crate::export::Settings,
    pub empty_message: Option<&'static str>,
    headers: Vec<Column>,
    pub selection: Option<Selection>,
    drag: Option<Point<Pixels>>,
    drag_hitbox: Option<HitboxId>,
    drag_task: Option<Task<()>>,
    ui_scale: f32,
}
impl Default for Results {
    fn default() -> Self {
        Self {
            columns: vec![],
            rows: crate::export::Rows::default(),
            pagination: Pagination::default(),
            export: crate::export::Settings::default(),
            empty_message: None,
            headers: vec![],
            selection: None,
            drag: None,
            drag_hitbox: None,
            drag_task: None,
            ui_scale: 1.,
        }
    }
}
impl Results {
    fn px(&self, value: f32) -> Pixels {
        px(self.ui_scale * value)
    }
    /// Set the interface scale and size the columns again. Call this also
    /// after a change of the interface font.
    pub fn set_ui_scale(&mut self, scale: f32, cx: &App) {
        self.ui_scale = scale;
        if !self.columns.is_empty() {
            self.schema(self.columns.clone(), cx);
        }
    }
    pub fn query_event(&mut self, event: &Event, cancelling: bool) -> bool {
        let message = match event {
            Event::Ready { limited: true, .. } => Some("No rows fit within the preview limit"),
            Event::Ready { .. } if cancelling => Some("Fetching stopped before any rows arrived"),
            Event::Ready { .. } if self.columns.is_empty() => {
                Some("Statement completed without a result set")
            }
            Event::Ready { .. } => Some("Query returned no rows"),
            Event::Cancelled => Some("Query cancelled before any rows arrived"),
            Event::Error { .. } => Some("Query failed before any rows arrived"),
            _ => None,
        };
        if let Some(message) = message {
            self.empty_message = Some(message);
            return true;
        }
        false
    }

    pub fn schema(&mut self, columns: Vec<DataColumn>, cx: &App) {
        let scale = self.ui_scale;
        let header = HeaderMeasure::new(scale, cx);
        // At 100% scale, the inner 4px inset keeps the original 6px text offset.
        let padding = Edges {
            top: self.px(3.),
            bottom: self.px(3.),
            left: self.px(2.),
            right: self.px(2.),
        };
        self.headers = vec![
            Column::new("row", "#")
                .width(self.px(48.))
                .paddings(padding)
                .fixed_left()
                .movable(false),
        ];
        self.headers
            .extend(columns.iter().enumerate().map(|(i, c)| {
                let width = match c.data_type.as_str() {
                    "TIMESTAMP" => 190.,
                    "STRING" => 180.,
                    _ => 120.,
                };
                Column::new(i.to_string(), c.name.clone())
                    .width(px(scale * width).max(header.width(c)))
                    .paddings(padding)
                    .movable(false)
            }));
        self.columns = columns;
    }
    pub fn clear(&mut self) {
        // The next timer turn releases this gesture's capture after a reset.
        let drag_task = self.drag_task.take();
        *self = Self {
            ui_scale: self.ui_scale,
            export: self.export.clone(),
            drag_task,
            ..Self::default()
        };
    }

    /// The rows of the current page.
    fn page(&self) -> std::ops::Range<usize> {
        self.pagination.range(self.rows.len())
    }

    /// Select one cell, or extend the selection to it from the anchor.
    pub fn select_cell(&mut self, row: usize, column: usize, extend: bool) {
        self.selection = Some(match self.selection {
            Some(selection) if extend => Selection {
                anchor: selection.anchor,
                focus: (row, column),
            },
            _ => Selection::cell(row, column),
        });
    }

    /// Select whole rows: one row, or the rows from the anchor to `row`.
    pub fn select_rows(&mut self, row: usize, extend: bool) {
        let Some(last) = self.columns.len().checked_sub(1) else {
            return;
        };
        let anchor = match self.selection {
            Some(selection) if extend => selection.anchor.0,
            _ => row,
        };
        self.selection = Some(Selection {
            anchor: (anchor, 0),
            focus: (row, last),
        });
    }

    /// Select whole columns of the current page: one column, or the columns
    /// from the anchor to `column`.
    pub fn select_columns(&mut self, column: usize, extend: bool) {
        let page = self.page();
        if page.is_empty() {
            return;
        }
        let anchor = match self.selection {
            Some(selection) if extend => selection.anchor.1,
            _ => column,
        };
        self.selection = Some(Selection {
            anchor: (page.start, anchor),
            focus: (page.end - 1, column),
        });
    }

    /// Select all cells of the current page.
    pub fn select_page(&mut self) {
        let page = self.page();
        if let (false, Some(last)) = (page.is_empty(), self.columns.len().checked_sub(1)) {
            self.selection = Some(Selection {
                anchor: (page.start, 0),
                focus: (page.end - 1, last),
            });
        }
    }

    /// Move the focus cell within the current page and return it. Without
    /// `extend`, the selection becomes the focus cell. Without a selection,
    /// the first cell of the page becomes selected.
    pub fn move_focus(
        &mut self,
        rows: isize,
        columns: isize,
        extend: bool,
    ) -> Option<(usize, usize)> {
        let page = self.page();
        let last = self.columns.len().checked_sub(1)?;
        if page.is_empty() {
            return None;
        }
        let Some(selection) = self.selection else {
            self.selection = Some(Selection::cell(page.start, 0));
            return Some((page.start, 0));
        };
        let row = selection
            .focus
            .0
            .saturating_add_signed(rows)
            .clamp(page.start, page.end - 1);
        let column = selection.focus.1.saturating_add_signed(columns).min(last);
        self.select_cell(row, column, extend);
        Some((row, column))
    }

    /// The stored value of a cell, or `None` for a null.
    fn value(&self, row: usize, column: usize) -> Option<&str> {
        self.rows
            .get(row)
            .and_then(|values| values.get(column))
            .and_then(|value| value.as_deref())
    }

    /// The text that Copy puts on the clipboard. One cell copies its value.
    /// A range copies tab-separated values without a header, which pastes
    /// into a spreadsheet as cells. Values with a tab, a line break, or a
    /// quote are quoted, as spreadsheets expect.
    pub fn selection_text(&self) -> Option<String> {
        let selection = self.selection?;
        if selection.is_cell() {
            let (row, column) = selection.focus;
            return Some(self.value(row, column).unwrap_or("NULL").to_owned());
        }
        let lines: Vec<String> = selection
            .rows()
            .map(|row| {
                selection
                    .columns()
                    .map(|column| tsv_field(self.value(row, column).unwrap_or("NULL")))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect();
        Some(lines.join("\n"))
    }

    fn empty_state(&self, cx: &App) -> Div {
        super::panel_empty_state(
            self.empty_message
                .unwrap_or("Run a query to preview its results"),
            cx,
        )
    }
}
/// Measures the name and the data type of a column header, so that a narrow
/// default width does not cut the data type.
struct HeaderMeasure {
    // A cache of its own, because the measure runs outside a window.
    text: WindowTextSystem,
    font: Font,
    rem: Pixels,
    scale: f32,
}
impl HeaderMeasure {
    fn new(scale: f32, cx: &App) -> Self {
        Self {
            text: WindowTextSystem::new(cx.text_system().clone()),
            font: font(cx.theme().font_family.clone()),
            rem: px(14. * scale),
            scale,
        }
    }
    /// The width of the text at 12/14 rem, or at 10/14 rem when `small`.
    fn text(&self, text: &str, small: bool) -> Pixels {
        let size = self.rem * if small { 10. / 14. } else { 12. / 14. };
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.text.layout_line(text, size, &[run], None).width
    }
    /// The column width that shows the full header, including the paddings
    /// of the table cell and of `render_th`.
    fn width(&self, column: &DataColumn) -> Pixels {
        let scale = self.scale;
        let name = self.text(&column.name, false);
        let data_type = self.text(&column.data_type, true);
        // Two `px_1` insets and one `gap_2` are 1 rem, the column paddings
        // are 4px, and the table keeps 6px less the right padding for its
        // sort icon. Two more pixels absorb the cell border and rounding.
        let cell = px(scale * 4.) + (px(6.) - px(scale * 2.)).max(px(0.)) + px(2.);
        name + data_type + self.rem + cell
    }
}

impl TableDelegate for Results {
    fn columns_count(&self, _: &App) -> usize {
        self.headers.len()
    }
    fn rows_count(&self, _: &App) -> usize {
        self.pagination.range(self.rows.len()).len()
    }
    fn column(&self, c: usize, _: &App) -> Column {
        self.headers[c].clone()
    }
    fn render_tr(
        &mut self,
        row: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        // The first row's outline sits inside the row; later outlines overlap
        // the preceding border. Reserve that pixel below the header as well.
        div().id(("row", row)).when(row == 0, |el| el.pt(px(1.)))
    }
    fn render_th(
        &mut self,
        c: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .id(("column-header", c))
            .test_support()
            .role(Role::ColumnHeader)
            .flex()
            .items_center()
            .gap_2()
            .size_full()
            .px_1()
            .overflow_hidden()
            .text_size(rems(12. / 14.))
            .aria_label(self.headers[c].name.clone())
            .on_click(cx.listener(move |state, event: &ClickEvent, window, cx| {
                let data = state.delegate_mut();
                if c == 0 {
                    data.select_page();
                } else {
                    data.select_columns(c - 1, event.modifiers().shift);
                }
                window.focus(&state.focus_handle(cx), cx);
                cx.notify();
            }))
            .child(self.headers[c].name.clone())
            .when(c > 0, |el| {
                el.child(
                    div()
                        .text_size(rems(10. / 14.))
                        .text_color(cx.theme().muted_foreground)
                        .child(self.columns[c - 1].data_type.clone()),
                )
            })
    }
    fn render_td(
        &mut self,
        r: usize,
        c: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let r = self.pagination.range(self.rows.len()).start + r;
        let number = (r + 1).to_string();
        let value = if c == 0 {
            Some(number.as_str())
        } else {
            self.rows[r].get(c - 1).and_then(|value| value.as_deref())
        };
        let null = value.is_none();
        let display: String = value.unwrap_or("NULL").chars().take(500).collect();
        let selection = self.selection.filter(|_| c > 0);
        let selected = selection.is_some_and(|s| s.contains(r, c - 1));
        // In a range, the focus cell also shows a ring.
        let focus = selection.is_some_and(|s| !s.is_cell() && s.focus == (r, c - 1));
        div()
            .id(("cell", c))
            .test_support()
            .role(Role::Cell)
            .aria_label(display.clone())
            .size_full()
            .px_1()
            .rounded_sm()
            .flex()
            .items_center()
            .text_size(rems(12. / 14.))
            .overflow_hidden()
            .text_color(cx.theme().foreground)
            .when(c == 0 || null, |el| {
                el.text_color(cx.theme().muted_foreground)
            })
            .when(selected, |el| {
                el.bg(cx.theme().selection)
                    .text_color(cx.theme().foreground)
            })
            .when(focus, |el| el.border_1().border_color(cx.theme().ring))
            .child(div().truncate().child(display))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |state, event: &MouseDownEvent, window, cx| {
                    let data = state.delegate_mut();
                    if c == 0 {
                        data.select_rows(r, event.modifiers.shift);
                    } else {
                        data.select_cell(r, c - 1, event.modifiers.shift);
                        data.drag = Some(event.position);
                        if let Some(hitbox) = data.drag_hitbox {
                            window.capture_pointer(hitbox);
                        }
                        start_drag(state, window, cx);
                    }
                    window.focus(&state.focus_handle(cx), cx);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |state, _, window, cx| {
                    let data = state.delegate_mut();
                    // A right-click inside the selection keeps it for the menu.
                    let inside = data.selection.is_some_and(|s| {
                        if c == 0 {
                            s.rows().contains(&r)
                        } else {
                            s.contains(r, c - 1)
                        }
                    });
                    if !inside {
                        if c == 0 {
                            data.select_rows(r, false);
                        } else {
                            data.select_cell(r, c - 1, false);
                        }
                    }
                    window.focus(&state.focus_handle(cx), cx);
                    cx.notify();
                }),
            )
    }
    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let row = self.pagination.range(self.rows.len()).start + row;
        let label = match self.selection {
            Some(selection) if selection.is_cell() && selection.focus.0 == row => "Copy cell",
            Some(selection) if !selection.is_cell() && selection.rows().contains(&row) => {
                "Copy selection"
            }
            _ => "",
        };
        let selection = self.selection_text().filter(|_| !label.is_empty());
        let text = self.rows[row]
            .iter()
            .map(|v| v.as_deref().unwrap_or("NULL"))
            .collect::<Vec<_>>()
            .join("\t");
        let source =
            crate::export::Snapshot::new(&self.columns, &self.rows).map(std::sync::Arc::new);
        let range = self.selection.map(|selection| {
            (
                *selection.rows().start()..selection.rows().end() + 1,
                selection.columns(),
            )
        });
        let mut csv = self.export.csv.clone();
        csv.header = true;
        if csv.separator == crate::export::csv::Separator::Tab {
            csv.separator = crate::export::csv::Separator::Comma;
        }
        let settings = self.export.clone();
        let menu = if let Ok(source) = source {
            let submenu = PopupMenu::build(window, cx, move |menu, _, _| {
                [
                    (
                        "CSV",
                        crate::export::Settings {
                            format: crate::export::Format::Csv,
                            csv: csv.clone(),
                            ..settings.clone()
                        },
                    ),
                    (
                        "TSV",
                        crate::export::Settings {
                            csv: crate::export::csv::Preset::Tsv.options(),
                            ..crate::export::Settings::default()
                        },
                    ),
                    (
                        "Markdown",
                        crate::export::Settings {
                            format: crate::export::Format::Markdown,
                            ..settings.clone()
                        },
                    ),
                    (
                        "JSON",
                        crate::export::Settings {
                            format: crate::export::Format::Json,
                            ..settings.clone()
                        },
                    ),
                ]
                .into_iter()
                .fold(menu, |menu, (label, options)| {
                    let source = source.clone();
                    let range = range.clone();
                    menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
                        super::export_dialog::copy_format(
                            source.clone(),
                            range.clone(),
                            options.clone(),
                            window,
                            cx,
                        );
                    }))
                })
            });
            menu.item(PopupMenuItem::submenu("Copy as", submenu))
        } else {
            menu
        };
        menu.when_some(selection, |menu, text| {
            menu.item(
                PopupMenuItem::new(label)
                    .on_click(move |_, _, cx| super::export_dialog::copy_text(text.clone(), cx)),
            )
        })
        .item(
            PopupMenuItem::new("Copy row")
                .on_click(move |_, _, cx| super::export_dialog::copy_text(text.clone(), cx)),
        )
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        self.empty_state(cx)
    }
}

/// Clear the cell selection after a click outside the results area.
pub fn selection_boundary(table: &Entity<TableState<Results>>) -> Div {
    let table = table.clone();
    div().on_mouse_down_out(move |_, window, cx| {
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) {
            return;
        }
        table.update(cx, |state, cx| {
            if state.delegate().selection.is_some() || state.selected_row().is_some() {
                state.delegate_mut().selection = None;
                state.delegate_mut().drag = None;
                state.clear_selection(cx);
            }
        });
    })
}

/// Handle horizontal wheel motion before the row list consumes the event.
/// Vertical motion stays with the virtual list; Shift-wheel also scrolls columns.
pub fn horizontal_scroll(table: &Entity<TableState<Results>>, scale: f32) -> impl IntoElement {
    let table = table.clone();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                if !phase.capture() || !bounds.contains(&event.position) {
                    return;
                }
                let delta = event.delta.pixel_delta(px(24. * scale));
                let dx = if event.modifiers.shift && delta.x == px(0.) {
                    delta.y
                } else {
                    delta.x
                };
                if dx == px(0.) || (!event.modifiers.shift && dx.abs() < delta.y.abs()) {
                    return;
                }
                table.update(cx, |state, cx| {
                    let handle = &state.horizontal_scroll_handle;
                    let mut offset = handle.offset();
                    offset.x += dx;
                    handle.set_offset(offset);
                    cx.notify();
                });
                cx.stop_propagation();
            });
        },
    )
    .absolute()
    .size_full()
    .inset_0()
}

/// Capture moves and release even after the pointer leaves a virtualized cell.
fn drag_selection(table: &Entity<TableState<Results>>) -> impl IntoElement {
    let geometry = table.downgrade();
    let events = table.downgrade();
    canvas(
        move |bounds, window, cx| {
            let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
            let _ = geometry.update(cx, |state, _| {
                state.delegate_mut().drag_hitbox = Some(hitbox.id)
            });
        },
        move |_, _, window, _| {
            let moves = events.clone();
            window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                if !phase.capture() {
                    return;
                }
                let _ = moves.update(cx, |state, cx| {
                    if state.delegate().drag.is_none() {
                        return;
                    }
                    if event.pressed_button != Some(MouseButton::Left) {
                        state.delegate_mut().drag = None;
                        window.release_pointer();
                        return;
                    }
                    state.delegate_mut().drag = Some(event.position);
                    update_drag(state, event.position, cx);
                    cx.stop_propagation();
                });
            });
            let release = events;
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase.capture() && event.button == MouseButton::Left {
                    let _ = release.update(cx, |state, cx| {
                        if state.delegate_mut().drag.take().is_some() {
                            update_drag(state, event.position, cx);
                            state.delegate_mut().drag_task = None;
                            window.release_pointer();
                        }
                    });
                }
            });
        },
    )
    .absolute()
    .size_full()
    .inset_0()
}

fn update_drag(
    state: &mut TableState<Results>,
    position: Point<Pixels>,
    cx: &mut Context<TableState<Results>>,
) {
    if let Some((row, column)) = state.cell_at_position(position, cx) {
        let data = state.delegate_mut();
        let focus = (data.page().start + row, column.saturating_sub(1));
        if let Some(selection) = data.selection.as_mut()
            && selection.focus != focus
        {
            selection.focus = focus;
            cx.notify();
        }
    }
}

/// Keep scrolling while a held pointer stays at an edge, even without new moves.
fn start_drag(
    state: &mut TableState<Results>,
    window: &mut Window,
    cx: &mut Context<TableState<Results>>,
) {
    let capture = window.captured_hitbox();
    state.delegate_mut().drag_task = Some(cx.spawn_in(window, async move |weak, cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(16))
                .await;
            let keep = weak.update_in(cx, |state, window, cx| {
                let Some(position) = state.delegate().drag else {
                    if capture.is_some() && window.captured_hitbox() == capture {
                        window.release_pointer();
                    }
                    return false;
                };
                if window.captured_hitbox() != capture {
                    state.delegate_mut().drag = None;
                    return false;
                }
                if !state.focus_handle(cx).contains_focused(window, cx)
                    || window.has_active_dialog(cx)
                    || window.has_active_sheet(cx)
                    || !window.is_window_active()
                {
                    state.delegate_mut().drag = None;
                    window.release_pointer();
                    return false;
                }
                let bounds = state.body_bounds();
                // Match the table's zoom: a row-height edge band, with a bounded step.
                let row_height = state.row_height();
                let edge = |value: Pixels, low: Pixels, high: Pixels| {
                    if value < low + row_height {
                        1.
                    } else if value > high - row_height {
                        -1.
                    } else {
                        0.
                    }
                };
                let delta = point(
                    row_height * 0.5 * edge(position.x, bounds.left(), bounds.right()),
                    row_height * 0.3 * edge(position.y, bounds.top(), bounds.bottom()),
                );
                if state.scroll_by(delta, cx) {
                    update_drag(state, position, cx);
                    cx.notify();
                }
                true
            });
            if !matches!(keep, Ok(true)) {
                break;
            }
        }
    }));
}

pub(super) fn view(
    table: &Entity<TableState<Results>>,
    modal: bool,
    scale: f32,
    cx: &App,
) -> AnyElement {
    let data = table.read(cx).delegate();
    if data.columns.is_empty() {
        return data.empty_state(cx).into_any_element();
    }

    let keys = keyboard(table);
    keys.size_full()
        .min_h_0()
        .min_w_0()
        .relative()
        .overflow_hidden()
        .child(
            DataTable::new(table)
                .small()
                .stripe(true)
                .bordered(false)
                .scrollbar_visible(true, true),
        )
        .when(!modal, |el| {
            el.child(horizontal_scroll(table, scale))
                .child(drag_selection(table))
        })
        .into_any_element()
}

/// Quote a value for tab-separated text when it contains a separator.
fn tsv_field(value: &str) -> Cow<'_, str> {
    if value.contains(['\t', '\n', '\r', '"']) {
        Cow::Owned(format!("\"{}\"", value.replace('"', "\"\"")))
    } else {
        Cow::Borrowed(value)
    }
}

/// Move the focus cell and scroll it into view.
fn move_focus(
    table: &Entity<TableState<Results>>,
    rows: isize,
    columns: isize,
    extend: bool,
    cx: &mut App,
) {
    table.update(cx, |state, cx| {
        let start = state.delegate().page().start;
        if let Some((row, column)) = state.delegate_mut().move_focus(rows, columns, extend) {
            state
                .vertical_scroll_handle
                .scroll_to_item(row - start, ScrollStrategy::Nearest);
            state.scroll_to_col(column + 1, cx);
        }
        cx.notify();
    });
}

/// The rows that Page Up and Page Down move: the visible rows less one.
fn page_step(table: &Entity<TableState<Results>>, cx: &App) -> isize {
    let rows = table.read(cx).visible_range().rows().len();
    rows.saturating_sub(1).max(1) as isize
}

/// Whether the keys belong to the table. The open context menu of the table
/// is inside the results view too; while it has the focus, its keys stay
/// with the menu.
fn table_keys(window: &Window) -> bool {
    !window
        .context_stack()
        .iter()
        .any(|context| context.contains("PopupMenu"))
}

/// The keyboard of the results table. The handlers run before the table's
/// own handlers, so the table keeps no row or column selection of its own.
fn keyboard(table: &Entity<TableState<Results>>) -> Div {
    macro_rules! step {
        ($action:ty, $rows:expr, $columns:expr, $extend:expr) => {{
            let table = table.clone();
            move |_: &$action, window: &mut Window, cx: &mut App| {
                if table_keys(window) {
                    cx.stop_propagation();
                    move_focus(&table, $rows, $columns, $extend, cx);
                }
            }
        }};
    }
    let far = isize::MAX / 2;
    let page_up = table.clone();
    let page_down = table.clone();
    let cancel = table.clone();
    let copy = table.clone();
    let all = table.clone();
    div()
        .key_context(CONTEXT)
        .capture_action(step!(SelectUp, -1, 0, false))
        .capture_action(step!(SelectDown, 1, 0, false))
        .capture_action(step!(SelectPrevColumn, 0, -1, false))
        .capture_action(step!(SelectNextColumn, 0, 1, false))
        .capture_action(step!(SelectFirst, 0, -far, false))
        .capture_action(step!(SelectLast, 0, far, false))
        .capture_action(step!(ExtendSelectionUp, -1, 0, true))
        .capture_action(step!(ExtendSelectionDown, 1, 0, true))
        .capture_action(step!(ExtendSelectionLeft, 0, -1, true))
        .capture_action(step!(ExtendSelectionRight, 0, 1, true))
        .capture_action(move |_: &SelectPageUp, window, cx| {
            if table_keys(window) {
                cx.stop_propagation();
                let step = page_step(&page_up, cx);
                move_focus(&page_up, -step, 0, false, cx);
            }
        })
        .capture_action(move |_: &SelectPageDown, window, cx| {
            if table_keys(window) {
                cx.stop_propagation();
                let step = page_step(&page_down, cx);
                move_focus(&page_down, step, 0, false, cx);
            }
        })
        .capture_action(move |_: &Cancel, window, cx| {
            // Escape clears a selection; without one, it goes on to the window.
            if !table_keys(window) {
                return;
            }
            cancel.update(cx, |state, cx| {
                if state.delegate_mut().selection.take().is_some() {
                    state.delegate_mut().drag = None;
                    state.delegate_mut().drag_task = None;
                    window.release_pointer();
                    cx.stop_propagation();
                    cx.notify();
                }
            });
        })
        .on_action(move |_: &Copy, window, cx| {
            if !table_keys(window) {
                return;
            }
            if let Some(text) = copy.read(cx).delegate().selection_text() {
                super::export_dialog::copy_text(text, cx);
            }
        })
        .on_action(move |_: &SelectAll, window, cx| {
            if !table_keys(window) {
                return;
            }
            all.update(cx, |state, cx| {
                state.delegate_mut().select_page();
                cx.notify();
            });
        })
}

/// Move within downloaded results and reset selection and vertical position.
pub fn select_page(table: &Entity<TableState<Results>>, page: usize, cx: &mut App) {
    table.update(cx, |state, cx| {
        let data = state.delegate_mut();
        if data.pagination.select(page, data.rows.len()) {
            data.selection = None;
            data.drag = None;
            state.clear_selection(cx);
            state.scroll_to_row(0, cx);
            cx.notify();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{DataColumn, Event, Results};
    use gpui_kit::{TestAppContext, px};

    fn column(name: &str, data_type: &str) -> DataColumn {
        DataColumn {
            name: name.into(),
            data_type: data_type.into(),
        }
    }

    #[gpui_kit::test]
    fn scaling_preserves_page_and_query_state_and_clear_preserves_scale(cx: &mut TestAppContext) {
        cx.update(crate::ui::init);
        cx.update(|cx| {
            let mut results = Results::default();
            results.schema(vec![column("value", "STRING")], cx);
            results.rows = vec![vec![Some("value".into())]; 1250].into();
            assert!(results.pagination.select(1, results.rows.len()));
            results.selection = Some(super::Selection::cell(1000, 0));
            results.query_event(&Event::Cancelled, false);

            results.set_ui_scale(1.5, cx);
            assert_eq!(results.pagination.range(results.rows.len()), 1000..1250);
            assert_eq!(results.selection, Some(super::Selection::cell(1000, 0)));
            assert_eq!(
                results.empty_message,
                Some("Query cancelled before any rows arrived")
            );
            assert_eq!(results.px(48.), px(72.));

            results.clear();
            assert_eq!(results.ui_scale, 1.5);
            assert_eq!(results.pagination.page(), 0);
            assert!(results.rows.is_empty());
            assert!(results.empty_message.is_none());
            assert!(results.selection.is_none());
        });
    }

    #[test]
    fn tab_separated_fields_are_quoted_only_when_needed() {
        assert_eq!(super::tsv_field("plain"), "plain");
        assert_eq!(super::tsv_field("a\tb"), "\"a\tb\"");
        assert_eq!(super::tsv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(super::tsv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[gpui_kit::test]
    fn selection_moves_and_extends_within_the_page(cx: &mut TestAppContext) {
        cx.update(crate::ui::init);
        cx.update(|cx| {
            let mut results = Results::default();
            results.schema(vec![column("a", "STRING"), column("b", "STRING")], cx);
            results.rows = (0..1250)
                .map(|i| vec![Some(format!("a{i}")), (i % 2 == 0).then(|| format!("b{i}"))])
                .collect();
            assert!(results.pagination.select(1, results.rows.len()));

            // Without a selection, a move selects the first cell of the page.
            assert_eq!(results.move_focus(1, 0, false), Some((1000, 0)));
            // Moves stop at the page and column edges.
            assert_eq!(results.move_focus(-5, -5, false), Some((1000, 0)));
            assert_eq!(results.move_focus(1000, 1000, false), Some((1249, 1)));
            assert_eq!(results.selection, Some(super::Selection::cell(1249, 1)));

            results.select_cell(1001, 0, false);
            results.move_focus(1, 1, true);
            let selection = results.selection.unwrap();
            assert_eq!(selection.anchor, (1001, 0));
            assert_eq!(selection.rows(), 1001..=1002);
            assert_eq!(selection.columns(), 0..=1);
            assert_eq!(
                results.selection_text().as_deref(),
                Some("a1001\tNULL\na1002\tb1002")
            );

            // Whole columns and the page stay within the current page.
            results.select_columns(1, false);
            assert_eq!(results.selection.unwrap().rows(), 1000..=1249);
            results.select_columns(0, true);
            assert_eq!(results.selection.unwrap().columns(), 0..=1);
            results.select_rows(1005, false);
            results.select_rows(1003, true);
            let selection = results.selection.unwrap();
            assert_eq!(selection.rows(), 1003..=1005);
            assert_eq!(selection.columns(), 0..=1);
            results.select_page();
            assert_eq!(results.selection_text().unwrap().lines().count(), 250);

            // One cell copies its raw value, and a null copies NULL.
            results.select_cell(1001, 1, false);
            assert_eq!(results.selection_text().as_deref(), Some("NULL"));
        });
    }

    #[gpui_kit::test]
    fn columns_widen_to_show_the_full_header(cx: &mut TestAppContext) {
        cx.update(crate::ui::init);
        cx.update(|cx| {
            let mut results = Results::default();
            results.schema(
                vec![
                    column("id", "INT"),
                    column("paid_bookings", "BIGINT"),
                    column("a_much_longer_column_name", "BIGINT"),
                ],
                cx,
            );
            let widths: Vec<_> = results.headers.iter().map(|c| c.width).collect();
            assert_eq!(widths[1], px(120.));
            assert!(widths[2] > px(120.));
            assert!(widths[3] > widths[2]);

            let header = super::HeaderMeasure::new(1., cx);
            let text =
                header.text("a_much_longer_column_name", false) + header.text("BIGINT", true);
            assert!(widths[3] > text);

            results.set_ui_scale(2., cx);
            assert!(results.headers[3].width > widths[3] * 1.5);
        });
    }
}
