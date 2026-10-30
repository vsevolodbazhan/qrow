use super::*;
use gpui_kit::component::{h_flex, popover::Popover, tag::Tag, v_flex};

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
        let data = tab.table.read(cx).delegate();
        let range = data.pagination.range(data.rows.len());
        Self {
            range: if range.is_empty() {
                "0 rows".into()
            } else {
                format!("Rows {}–{}", range.start + 1, range.end)
            },
            loaded: data.rows.len(),
            columns: data.columns.len(),
            elapsed: tab
                .elapsed
                .map(|elapsed| format!("{:.2} s", elapsed.as_secs_f64())),
        }
    }

    fn tags(&self, range_only: bool, cx: &App) -> impl IntoElement {
        h_flex()
            .id("result-count")
            .test_support()
            .flex_shrink_0()
            .gap_2()
            .child(Self::tag(
                "result-range",
                &self.range,
                &self.range,
                VISIBLE_ROWS,
                cx,
            ))
            .when(!range_only, |row| {
                row.child(Self::tag(
                    "result-loaded",
                    &format!("{} loaded", self.loaded),
                    &format!("{} loaded", self.loaded),
                    LOADED_ROWS,
                    cx,
                ))
                .child(Self::tag(
                    "result-columns",
                    &self.columns_label(),
                    &self.columns_label(),
                    COLUMNS,
                    cx,
                ))
                .when_some(self.elapsed.as_ref(), |row, elapsed| {
                    row.child(Self::tag(
                        "result-elapsed",
                        elapsed,
                        &format!("Elapsed: {elapsed}"),
                        QUERY_DURATION,
                        cx,
                    ))
                })
            })
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

    fn tag(
        id: &'static str,
        text: &str,
        name: &str,
        title: &'static str,
        cx: &App,
    ) -> impl IntoElement {
        div()
            .id(id)
            .test_support()
            .role(Role::Label)
            .whitespace_nowrap()
            .aria_label(name.to_owned())
            .tooltip(move |window, cx| StatusTooltip::new(title, "").build(None, window, cx))
            .child(Self::neutral_tag(cx).child(text.to_owned()))
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
        ResultToolbar {
            frame: div().w_full().h_10().flex_shrink_0().into_any_element(),
            full: self.result_toolbar_row(false, true, cx),
            compact: self.result_toolbar_row(true, true, cx),
            minimal: self.result_toolbar_row(true, false, cx),
        }
    }

    fn result_toolbar_row(
        &self,
        compact: bool,
        show_range: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab = &self.tabs[self.active];
        let data = tab.table.read(cx).delegate();
        let page = data.pagination.page();
        let pages = data.pagination.pages(data.rows.len());
        let export_disabled = data.columns.is_empty();
        let draining = tab.download.is_some() || tab.cursor == crate::worker::Cursor::Draining;
        let summary = ResultSummary::new(tab, cx);
        let page_label = format!("Page {}", page + 1);
        h_flex()
            .id("query-footer")
            .test_support()
            .w_full()
            .h_10()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(self.panel_switcher(cx))
            .when(show_range, |row| row.child(summary.tags(compact, cx)))
            .when(compact, |row| {
                let qrow = cx.entity().downgrade();
                let tab_id = tab.saved.id;
                row.child(
                    Popover::new(SharedString::from(format!(
                        "result-details-popover-{tab_id}"
                    )))
                    .trigger(
                        Button::new("result-details")
                            .ghost()
                            .small()
                            .icon(IconName::Info)
                            .accessibility_label("Show result details")
                            .tooltip("Show result details"),
                    )
                    .content(move |_, _, cx| {
                        let Some(qrow) = qrow.upgrade() else {
                            return div().into_any_element();
                        };
                        let Some(tab) =
                            qrow.read(cx).tabs.iter().find(|tab| tab.saved.id == tab_id)
                        else {
                            return div().into_any_element();
                        };
                        ResultSummary::new(tab, cx).details(cx).into_any_element()
                    }),
                )
            })
            .child(div().flex_1().min_w_0())
            .child(
                Button::new("export-results")
                    .small()
                    .ghost()
                    .icon(gpui_kit::assets::IconName::Download)
                    .tooltip("Export results…")
                    .accessibility_label("Export results…")
                    .disabled(export_disabled || draining)
                    .on_click(cx.listener(|this, _, window, cx| this.open_export(window, cx))),
            )
            .child(
                h_flex()
                    .flex_shrink_0()
                    .gap_2()
                    .child(
                        div()
                            .id("page-label")
                            .test_support()
                            .role(Role::Label)
                            .text_xs()
                            .whitespace_nowrap()
                            .aria_label(page_label.clone())
                            .child(page_label),
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
                            .disabled(draining || page + 1 >= pages && (!tab.more || tab.busy))
                            .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                        cx,
                    )),
            )
            .into_any_element()
    }
}

/// Selects a toolbar using its real intrinsic width in the current font and scale.
/// The result pane supplies the available width. No breakpoint or geometry cache
/// can become stale after a pane resize or a change to a result count.
struct ResultToolbar {
    frame: AnyElement,
    full: AnyElement,
    compact: AnyElement,
    minimal: AnyElement,
}

impl IntoElement for ResultToolbar {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for ResultToolbar {
    type RequestLayoutState = ();
    type PrepaintState = u8;

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.frame.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> u8 {
        let intrinsic = size(AvailableSpace::MaxContent, AvailableSpace::MaxContent);
        let mode = if self.full.layout_as_root(intrinsic, window, cx).width <= bounds.size.width {
            0
        } else if self.compact.layout_as_root(intrinsic, window, cx).width <= bounds.size.width {
            1
        } else {
            2
        };
        let row = match mode {
            0 => &mut self.full,
            1 => &mut self.compact,
            _ => &mut self.minimal,
        };
        row.prepaint_as_root(
            bounds.origin,
            bounds.size.map(AvailableSpace::Definite),
            window,
            cx,
        );
        mode
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        mode: &mut u8,
        window: &mut Window,
        cx: &mut App,
    ) {
        let row = match mode {
            0 => &mut self.full,
            1 => &mut self.compact,
            _ => &mut self.minimal,
        };
        row.paint(window, cx);
    }
}
