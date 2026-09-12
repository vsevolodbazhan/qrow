use gpui_kit::component::{
    ActiveTheme, Sizable,
    menu::{PopupMenu, PopupMenuItem},
    scroll::Scrollbar,
    table::{Column, DataTable, TableDelegate, TableState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use qrow::{
    model::{Column as DataColumn, Row},
    pagination::Pagination,
    worker::Event,
};

#[derive(Default)]
pub struct Results {
    pub columns: Vec<DataColumn>,
    pub rows: Vec<Row>,
    pub pagination: Pagination,
    pub empty_message: Option<&'static str>,
    headers: Vec<Column>,
    pub selected: Option<(usize, usize)>,
}
impl Results {
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

    pub fn schema(&mut self, columns: Vec<DataColumn>) {
        // The inner 4px inset leaves text at the table's original 6px offset.
        let padding = Edges {
            top: px(3.),
            bottom: px(3.),
            left: px(2.),
            right: px(2.),
        };
        self.headers = vec![
            Column::new("row", "#")
                .width(px(48.))
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
                    .width(px(width))
                    .paddings(padding)
                    .movable(false)
            }));
        self.columns = columns;
    }
    pub fn clear(&mut self) {
        *self = Self::default();
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
            .flex()
            .items_center()
            .gap_2()
            .size_full()
            .px_1()
            .overflow_hidden()
            .text_sm()
            .child(self.headers[c].name.clone())
            .when(c > 0, |el| {
                el.child(
                    div()
                        .text_xs()
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
        div()
            .id(("cell", c))
            .size_full()
            .px_1()
            .rounded_sm()
            .flex()
            .items_center()
            .text_sm()
            .overflow_hidden()
            .text_color(cx.theme().foreground)
            .when(c == 0 || null, |el| {
                el.text_color(cx.theme().muted_foreground)
            })
            .when(self.selected == Some((r, c)), |el| {
                el.bg(cx.theme().selection).text_color(rgb(0xf0f4fc))
            })
            .child(div().line_height(relative(1.)).truncate().child(display))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |s, _, _, cx| {
                    s.delegate_mut().selected = Some((r, c));
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |s, _, _, cx| {
                    s.delegate_mut().selected = Some((r, c));
                    cx.notify();
                }),
            )
    }
    fn context_menu(
        &mut self,
        row: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let row = self.pagination.range(self.rows.len()).start + row;
        let cell = self.selected.filter(|(r, _)| *r == row).map(|(_, c)| {
            if c == 0 {
                (row + 1).to_string()
            } else {
                self.rows[row][c - 1]
                    .clone()
                    .unwrap_or_else(|| "NULL".into())
            }
        });
        let text = self.rows[row]
            .iter()
            .map(|v| v.as_deref().unwrap_or("NULL"))
            .collect::<Vec<_>>()
            .join("\t");
        menu.when_some(cell, |menu, text| {
            menu.item(PopupMenuItem::new("Copy cell").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
            }))
        })
        .item(PopupMenuItem::new("Copy row").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
        }))
    }
    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(cx.theme().muted_foreground)
            .text_sm()
            .child(
                self.empty_message
                    .unwrap_or("Run a query to preview its results"),
            )
    }
}

/// Clear the cell selection after a click outside the results area.
pub fn selection_boundary(table: &Entity<TableState<Results>>) -> Div {
    let table = table.clone();
    div().on_mouse_down_out(move |_, _, cx| {
        table.update(cx, |state, cx| {
            if state.delegate().selected.is_some() || state.selected_row().is_some() {
                state.delegate_mut().selected = None;
                state.clear_selection(cx);
            }
        });
    })
}

/// Handle horizontal wheel motion before the row list consumes the event.
/// Vertical motion stays with the virtual list; Shift-wheel also scrolls columns.
pub fn horizontal_scroll(table: &Entity<TableState<Results>>) -> impl IntoElement {
    let table = table.clone();
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                if !phase.capture() || !bounds.contains(&event.position) {
                    return;
                }
                let delta = event.delta.pixel_delta(px(24.));
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

/// Reserve a scrollbar lane inside the results viewport. An overlay track can
/// otherwise fall outside the table's clipped container in the Kit layout.
pub(super) fn view(table: &Entity<TableState<Results>>, modal: bool, cx: &App) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .min_h_0()
        .min_w_0()
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .relative()
                .overflow_hidden()
                .child(
                    DataTable::new(table)
                        .small()
                        .stripe(true)
                        .bordered(false)
                        .scrollbar_visible(true, false),
                )
                .when(!modal, |el| el.child(horizontal_scroll(table))),
        )
        .child(div().h_3().w_full().flex_shrink_0().relative().child(
            Scrollbar::horizontal(&table.read(cx).horizontal_scroll_handle).viewport_from_layout(),
        ))
}

/// Move within downloaded results and reset selection and vertical position.
pub fn select_page(table: &Entity<TableState<Results>>, page: usize, cx: &mut App) {
    table.update(cx, |state, cx| {
        let data = state.delegate_mut();
        if data.pagination.select(page, data.rows.len()) {
            data.selected = None;
            state.clear_selection(cx);
            state.scroll_to_row(0, cx);
            cx.notify();
        }
    });
}
