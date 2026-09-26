use gpui_kit::component::{
    ActiveTheme, Sizable,
    menu::{PopupMenu, PopupMenuItem},
    table::{Column, DataTable, TableDelegate, TableState},
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use qrow::{
    model::{Column as DataColumn, Row},
    pagination::Pagination,
    worker::Event,
};

pub struct Results {
    pub columns: Vec<DataColumn>,
    pub rows: Vec<Row>,
    pub pagination: Pagination,
    pub empty_message: Option<&'static str>,
    headers: Vec<Column>,
    pub selected: Option<(usize, usize)>,
    ui_scale: f32,
}
impl Default for Results {
    fn default() -> Self {
        Self {
            columns: vec![],
            rows: vec![],
            pagination: Pagination::default(),
            empty_message: None,
            headers: vec![],
            selected: None,
            ui_scale: 1.,
        }
    }
}
impl Results {
    fn px(&self, value: f32) -> Pixels {
        px(self.ui_scale * value)
    }
    pub fn set_ui_scale(&mut self, scale: f32) {
        self.ui_scale = scale;
        if !self.columns.is_empty() {
            self.schema(self.columns.clone());
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

    pub fn schema(&mut self, columns: Vec<DataColumn>) {
        let scale = self.ui_scale;
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
                    .width(px(scale * width))
                    .paddings(padding)
                    .movable(false)
            }));
        self.columns = columns;
    }
    pub fn clear(&mut self) {
        *self = Self {
            ui_scale: self.ui_scale,
            ..Self::default()
        };
    }

    fn empty_state(&self, cx: &App) -> Div {
        super::panel_empty_state(
            self.empty_message
                .unwrap_or("Run a query to preview its results"),
            cx,
        )
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
            .role(Role::ColumnHeader)
            .flex()
            .items_center()
            .gap_2()
            .size_full()
            .px_1()
            .overflow_hidden()
            .text_size(rems(12. / 14.))
            .aria_label(self.headers[c].name.clone())
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
        div()
            .id(("cell", c))
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
            .when(self.selected == Some((r, c)), |el| {
                el.bg(cx.theme().selection)
                    .text_color(cx.theme().foreground)
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
            menu.item(PopupMenuItem::new("Copy Cell").on_click(move |_, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
            }))
        })
        .item(PopupMenuItem::new("Copy Row").on_click(move |_, _, cx| {
            cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
        }))
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

    div()
        .size_full()
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
        .when(!modal, |el| el.child(horizontal_scroll(table, scale)))
        .into_any_element()
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

#[cfg(test)]
mod tests {
    use super::{DataColumn, Event, Results};
    use gpui_kit::px;

    #[test]
    fn scaling_preserves_page_and_query_state_and_clear_preserves_scale() {
        let mut results = Results::default();
        results.schema(vec![DataColumn {
            name: "value".into(),
            data_type: "STRING".into(),
        }]);
        results.rows = vec![vec![Some("value".into())]; 1250];
        assert!(results.pagination.select(1, results.rows.len()));
        results.selected = Some((1000, 1));
        results.query_event(&Event::Cancelled, false);

        results.set_ui_scale(1.5);
        assert_eq!(results.pagination.range(results.rows.len()), 1000..1250);
        assert_eq!(results.selected, Some((1000, 1)));
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
        assert!(results.selected.is_none());
    }
}
