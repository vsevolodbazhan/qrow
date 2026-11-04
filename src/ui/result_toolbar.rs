use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::{h_flex, menu::DropdownMenu, popover::Popover, tag::Tag, v_flex};

const VISIBLE_ROWS: &str = "Visible Rows";
const LOADED_ROWS: &str = "Loaded Rows";
const COLUMNS: &str = "Columns";
const QUERY_DURATION: &str = "Query Duration";

struct ResultSummary {
    range: String,
    loaded: usize,
    columns: usize,
    elapsed: Option<String>,
}

impl ResultSummary {
    fn new(tab: &Tab, cx: &App) -> Self {
        let result = tab.shown_result();
        let data = result.table.read(cx).delegate();
        let range = data.pagination.range(data.rows.len());
        Self {
            range: if range.is_empty() {
                "0 rows".into()
            } else {
                format!("Rows {}–{}", range.start + 1, range.end)
            },
            loaded: data.rows.len(),
            columns: data.columns.len(),
            elapsed: result
                .elapsed
                .map(|elapsed| format!("{:.2} s", elapsed.as_secs_f64())),
        }
    }

    fn columns_label(&self) -> String {
        format!(
            "{} {}",
            self.columns,
            if self.columns == 1 {
                "column"
            } else {
                "columns"
            }
        )
    }

    fn neutral_tag(cx: &App) -> Tag {
        Tag::secondary().small().border_color(cx.theme().secondary)
    }

    fn details(&self, cx: &App) -> impl IntoElement {
        v_flex()
            .id("result-details-content")
            .test_support()
            .role(Role::Group)
            .aria_label("Result Details")
            .gap_2()
            .text_sm()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Result Details"),
            )
            .children(
                [
                    (
                        "result-detail-range",
                        VISIBLE_ROWS,
                        self.range.strip_prefix("Rows ").unwrap_or("0").to_owned(),
                    ),
                    ("result-detail-loaded", LOADED_ROWS, self.loaded.to_string()),
                    ("result-detail-columns", COLUMNS, self.columns.to_string()),
                ]
                .into_iter()
                .map(|(id, label, value)| Self::detail(id, label, value, cx)),
            )
            .when_some(self.elapsed.clone(), |column, elapsed| {
                column.child(Self::detail(
                    "result-detail-elapsed",
                    QUERY_DURATION,
                    elapsed,
                    cx,
                ))
            })
    }

    fn detail(id: &'static str, name: &'static str, value: String, cx: &App) -> impl IntoElement {
        h_flex()
            .id(id)
            .test_support()
            .role(Role::Label)
            .aria_label(format!("{name}: {value}"))
            .items_center()
            .justify_between()
            .gap_6()
            .child(name)
            .child(Self::neutral_tag(cx).child(value))
    }
}

impl Qrow {
    pub(super) fn result_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let tab = &self.tabs[self.active];
        let result = tab.shown_result();
        let (tab_id, result_id) = (tab.saved.id, result.id);
        let data = result.table.read(cx).delegate();
        let page = data.pagination.page();
        let pages = data.pagination.pages(data.rows.len());
        let draining = tab.live == Some(result_id)
            && (tab.download.is_some() || tab.cursor == crate::worker::Cursor::Draining);
        let summary = ResultSummary::new(tab, cx);
        let range = summary.range.strip_prefix("Rows ").unwrap_or("0");
        let mut description = format!(
            "{range} of {} loaded · {}",
            summary.loaded,
            summary.columns_label()
        );
        if let Some(elapsed) = &summary.elapsed {
            description.push_str(&format!(" · {elapsed}"));
        }
        if result.unfetched {
            description.push_str(" · Partial result");
        }
        let owner = cx.entity().downgrade();
        let menu_owner = owner.clone();
        let pinned = result.pinned;
        let can_pin = pinned || tab.pinned_results() < MAX_PINNED_RESULTS;
        let has_sql = result.sql.is_some();
        h_flex()
            .id("query-footer")
            .test_support()
            .w_full()
            .h_8()
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("result-count")
                    .test_support()
                    .role(Role::Label)
                    .aria_label(description.clone())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .tooltip(move |window, cx| {
                        StatusTooltip::new(
                            "Loaded preview",
                            "The count includes downloaded rows, not all rows on the server.",
                        )
                        .build(None, window, cx)
                    })
                    .child(description),
            )
            .child(
                Popover::new("result-details-popover")
                    .trigger(
                        Button::new("result-details")
                            .ghost()
                            .small()
                            .icon(IconName::Info)
                            .accessibility_label("Show result details")
                            .tooltip("Show result details"),
                    )
                    .content(move |_, _, cx| {
                        let Some(owner) = owner.upgrade() else {
                            return div().into_any_element();
                        };
                        let Some(tab) = owner
                            .read(cx)
                            .tabs
                            .iter()
                            .find(|tab| tab.saved.id == tab_id)
                        else {
                            return div().into_any_element();
                        };
                        ResultSummary::new(tab, cx).details(cx).into_any_element()
                    }),
            )
            .when(data.columns.len() > 8, |row| {
                row.child(
                    Button::new("find-result-column")
                        .ghost()
                        .small()
                        .icon(AssetIconName::Columns3)
                        .label("Columns…")
                        .tooltip("Find a result column")
                        .accessibility_label("Find a result column")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.open_column_picker(window, cx)),
                        ),
                )
            })
            .child(
                Button::new("export-results")
                    .outline()
                    .small()
                    .icon(AssetIconName::Download)
                    .label("Export…")
                    .accessibility_label("Export results…")
                    .disabled(data.columns.is_empty() || draining)
                    .on_click(cx.listener(|this, _, window, cx| this.open_export(window, cx))),
            )
            .child(
                Button::new("result-actions")
                    .ghost()
                    .small()
                    .icon(IconName::Ellipsis)
                    .accessibility_label("Result actions")
                    .tooltip("Result actions")
                    .dropdown_menu(move |menu, _, _| {
                        let copy = menu_owner.clone();
                        let pin = menu_owner.clone();
                        menu.item(
                            PopupMenuItem::new("Copy query")
                                .disabled(!has_sql)
                                .on_click(move |_, _, cx| {
                                    let _ = copy.update(cx, |this, cx| {
                                        this.copy_result_query(tab_id, result_id, cx)
                                    });
                                }),
                        )
                        .item(
                            PopupMenuItem::new(if pinned { "Unpin result" } else { "Pin result" })
                                .disabled(!can_pin)
                                .on_click(move |_, window, cx| {
                                    let _ = pin.update(cx, |this, cx| {
                                        this.toggle_result_pin(tab_id, result_id, window, cx)
                                    });
                                }),
                        )
                    }),
            )
            .child(button_pair::button_pair(
                "pagination-buttons",
                Button::new("previous-page")
                    .small()
                    .ghost()
                    .icon(IconName::ChevronLeft)
                    .tooltip("Previous page")
                    .accessibility_label("Previous page")
                    .disabled(page == 0 || draining)
                    .on_click(cx.listener(|this, _, _, cx| this.previous_page(cx))),
                Button::new("next-page")
                    .small()
                    .ghost()
                    .icon(IconName::ChevronRight)
                    .tooltip("Next page")
                    .accessibility_label("Next page")
                    .disabled(
                        draining || page + 1 >= pages && (!tab.can_fetch(result_id) || tab.busy),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                cx,
            ))
    }
}

impl Qrow {
    fn open_column_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::select::{SearchableVec, Select, SelectState};
        let table = self.tabs[self.active].shown_result().table.clone();
        let choices: Vec<String> = table
            .read(cx)
            .delegate()
            .columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                format!("{} · {} · {}", index + 1, column.name, column.data_type)
            })
            .collect();
        let search = cx.new(|cx| {
            SelectState::new(SearchableVec::new(choices.clone()), None, window, cx).searchable(true)
        });
        let subscription = cx.subscribe_in(
            &search,
            window,
            move |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                if let SelectEvent::Confirm(Some(value)) = event {
                    if let Some(index) = choices.iter().position(|choice| choice == value) {
                        this.column_picker = None;
                        window.close_dialog(cx);
                        table.update(cx, |table, cx| {
                            table.scroll_to_col(index + 1, cx);
                            window.focus(&table.focus_handle(cx), cx);
                        });
                        cx.notify();
                    }
                }
            },
        );
        self.column_picker = Some((search.clone(), subscription));
        let owner = cx.weak_entity();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Go to Column")
                .w_96()
                .child(
                    Select::new(&search)
                        .id("result-column-picker")
                        .focus_ring(false)
                        .w_full()
                        .search_placeholder("Find a column…")
                        .placeholder("Choose a column"),
                )
                .on_close({
                    let owner = owner.clone();
                    move |_, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.column_picker = None;
                            cx.notify();
                        });
                    }
                })
        });
        cx.notify();
    }
}
