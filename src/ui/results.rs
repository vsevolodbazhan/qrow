use gpui::prelude::FluentBuilder;
use gpui::*;
use gpui_component::{
    menu::{PopupMenu, PopupMenuItem},
    table::{Column, TableDelegate, TableState},
};
use qrow::model::{Column as DataColumn, Row};

#[derive(Default)]
pub struct Results {
    pub columns: Vec<DataColumn>,
    pub rows: Vec<Row>,
    headers: Vec<Column>,
    pub selected: Option<(usize, usize)>,
}
impl Results {
    pub fn schema(&mut self, columns: Vec<DataColumn>) {
        self.headers = vec![
            Column::new("row", "#")
                .width(px(48.))
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
        self.rows.len()
    }
    fn column(&self, c: usize, _: &App) -> &Column {
        &self.headers[c]
    }
    fn render_th(
        &mut self,
        c: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .flex()
            .items_center()
            .gap_2()
            .size_full()
            .overflow_hidden()
            .text_size(px(12.))
            .child(self.headers[c].name.clone())
            .when(c > 0, |el| {
                el.child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(0x7f899a))
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
        let value = if c == 0 {
            Some((r + 1).to_string())
        } else {
            self.rows[r].get(c - 1).cloned().flatten()
        };
        let null = value.is_none();
        let display: String = value
            .unwrap_or_else(|| "NULL".into())
            .chars()
            .take(500)
            .collect();
        div()
            .id(("cell", c))
            .size_full()
            .flex()
            .items_center()
            .text_size(px(12.))
            .overflow_hidden()
            .when(c == 0 || null, |el| el.text_color(rgb(0xb7c1d0)))
            .when(self.selected == Some((r, c)), |el| el.bg(rgb(0x30425c)))
            .child(div().truncate().child(display))
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
        _: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x7f899a))
            .text_size(px(13.))
            .child("Run a query to preview its results")
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
    .inset_0()
}
