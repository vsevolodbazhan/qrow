use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{
    Icon, h_flex,
    tab::{Tab as PanelTab, TabBar},
};

/// The height of the Logs and result tabs at 100% UI scale.
pub(super) const RESULT_TAB_BAR_HEIGHT: f32 = 30.;
/// The longest part of the query that a result tab tooltip shows.
const TOOLTIP_SQL_CHARS: usize = 120;

/// The name field of a result tab that the user renames.
pub(super) struct ResultEditor {
    tab: Uuid,
    result: u64,
    title: Entity<InputState>,
    error: Option<String>,
}

pub(super) const RESULT_RENAME: tab_view::RenameDialog = tab_view::RenameDialog {
    key: "result",
    title: "Rename Result",
    label: "Name",
    tooltip: "Rename result",
    form: |this| {
        this.result_form
            .as_ref()
            .map(|form| (&form.title, form.error.clone()))
    },
    submit: Qrow::rename_result,
    clear: |this| this.result_form = None,
};

/// The query on one line, for a tooltip. It reads only the start of the
/// SQL that the tooltip shows.
fn sql_summary(sql: &str) -> String {
    let mut line = String::new();
    let mut chars = 0;
    for word in sql.split_whitespace() {
        for c in (chars > 0).then_some(' ').into_iter().chain(word.chars()) {
            if chars == TOOLTIP_SQL_CHARS {
                line.push('…');
                return line;
            }
            line.push(c);
            chars += 1;
        }
    }
    line
}

impl Qrow {
    /// Logs and the result tabs of the active query tab.
    pub(super) fn result_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let height = self.ui_px(RESULT_TAB_BAR_HEIGHT);
        let menu_open = self.menu.is_some();
        let tab = &self.tabs[self.active];
        let selected = match tab.panel.selected {
            Panel::Output => 0,
            Panel::Results => {
                1 + tab
                    .results
                    .iter()
                    .position(|result| result.id == tab.shown_result().id)
                    .unwrap_or(0)
            }
        };
        let tab_id = tab.saved.id;
        let logs = PanelTab::new()
            .min_h(height)
            .max_h(height)
            .aria_label("Logs Panel")
            .child(
                div()
                    .id("output-panel-tab")
                    .test_support()
                    .aria_label("Logs")
                    .child("Logs"),
            );
        TabBar::new("result-tabs")
            .underline()
            .small()
            .h(height)
            .min_h(height)
            .max_h(height)
            .px_3()
            .selected_index(selected)
            .suffix(h_flex().h_full().child(self.results_toggle_button(cx)))
            .child(logs)
            .children(tab.results.iter().map(|result| {
                let id = result.id;
                let shown = id == tab.shown_result().id;
                let title = result.title.clone();
                let tooltip =
                    StatusTooltip::new(title.clone(), if result.pinned { "Pinned" } else { "" })
                        .when_some(result.sql.as_deref(), |tooltip, sql| {
                            tooltip.detail(sql_summary(sql))
                        });
                // Tests find the result that Results shows by a stable ID, also
                // while Logs is selected.
                let label = div()
                    .id(SharedString::from(format!("result-tab-{id}")))
                    .test_support()
                    .aria_label(title.clone())
                    .child(title.clone());
                let label = if shown {
                    div().id("results-panel-tab").test_support().child(label)
                } else {
                    label
                };
                PanelTab::new()
                    .min_h(height)
                    .max_h(height)
                    .aria_label(format!(
                        "{title}{}",
                        if result.pinned { ", pinned" } else { "" }
                    ))
                    // The tooltip would cover the context menu of the tab.
                    .map(|mut panel_tab| {
                        if !menu_open {
                            panel_tab
                                .interactivity()
                                .tooltip(move |window, cx| tooltip.build(None, window, cx));
                        }
                        panel_tab
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |_, event: &MouseDownEvent, window, cx| {
                            let position = event.position;
                            cx.defer_in(window, move |this, window, cx| {
                                this.open_result_menu(tab_id, id, position, window, cx)
                            });
                        }),
                    )
                    .when(result.pinned, |panel_tab| {
                        panel_tab.prefix(
                            div()
                                .pl_2()
                                .text_color(cx.theme().muted_foreground)
                                .child(Icon::new(AssetIconName::Pin).xsmall()),
                        )
                    })
                    .child(label)
                    .suffix(
                        h_flex().pr_1().child(
                            Button::new(SharedString::from(format!("close-result-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Close result")
                                .accessibility_label(format!("Close {title}"))
                                .disabled(!tab.can_close_result(id))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    cx.stop_propagation();
                                    this.close_result(tab_id, id, window, cx);
                                })),
                        ),
                    )
            }))
            .on_click(cx.listener(move |this, index: &usize, _, cx| {
                this.restore_results(cx);
                let Some(tab) = this.tabs.iter_mut().find(|tab| tab.saved.id == tab_id) else {
                    return;
                };
                match index
                    .checked_sub(1)
                    .and_then(|index| tab.results.get(index))
                {
                    Some(result) => {
                        tab.active_result = result.id;
                        tab.panel.user_select(Panel::Results);
                    }
                    None => tab.panel.user_select(Panel::Output),
                }
                cx.notify();
            }))
    }

    fn open_result_menu(
        &mut self,
        tab_id: Uuid,
        id: u64,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.iter().find(|tab| tab.saved.id == tab_id) else {
            return;
        };
        let Some(result) = tab.result(id) else {
            return;
        };
        let pinned = result.pinned;
        let can_pin = pinned || tab.pinned_results() < MAX_PINNED_RESULTS;
        let has_sql = result.sql.is_some();
        let can_close = tab.can_close_result(id);
        let rename = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.open_result_rename(tab_id, id, window, cx)
        });
        let pin = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.toggle_result_pin(tab_id, id, window, cx)
        });
        let copy =
            cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_result_query(tab_id, id, cx));
        let close = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.close_result(tab_id, id, window, cx)
        });
        self.open_context_menu(
            position,
            move |menu, _, _| {
                menu.item(PopupMenuItem::new("Rename…").on_click(rename))
                    .item(
                        PopupMenuItem::new(if pinned { "Unpin" } else { "Pin" })
                            .disabled(!can_pin)
                            .on_click(pin),
                    )
                    .item(
                        PopupMenuItem::new("Copy query")
                            .disabled(!has_sql)
                            .on_click(copy),
                    )
                    .separator()
                    .item(
                        PopupMenuItem::new("Close")
                            .disabled(!can_close)
                            .on_click(close),
                    )
            },
            window,
            cx,
        );
    }

    /// Pins or unpins a result tab. A run replaces only an unpinned result tab.
    pub(super) fn toggle_result_pin(
        &mut self,
        tab_id: Uuid,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.tabs.iter().position(|tab| tab.saved.id == tab_id) else {
            return;
        };
        let tab = &mut self.tabs[index];
        let pinned = tab.pinned_results();
        let Some(result) = tab.result_mut(id) else {
            return;
        };
        if !result.pinned && pinned >= MAX_PINNED_RESULTS {
            return;
        }
        result.pinned = !result.pinned;
        self.prepare_spare_result(index, window, cx);
        cx.notify();
    }

    /// Makes a table ready for the next run when every result tab is pinned.
    /// A run can start without a window, and a new table needs one.
    fn prepare_spare_result(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &self.tabs[index];
        if tab.spare_table.is_some() || tab.results.iter().any(|result| !result.pinned) {
            return;
        }
        let table = self.result_table(self.tabs[index].saved.profile, window, cx);
        self.tabs[index].spare_table = Some(table);
    }

    pub(super) fn copy_result_query(&self, tab_id: Uuid, id: u64, cx: &mut App) {
        if let Some(sql) = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == tab_id)
            .and_then(|tab| tab.result(id))
            .and_then(|result| result.sql.clone())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(sql));
        }
    }

    fn close_result(&mut self, tab_id: Uuid, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|tab| tab.saved.id == tab_id) else {
            return;
        };
        if self.dialog_open() || !self.tabs[index].can_close_result(id) {
            return;
        }
        // The menu names a result tab that is about to disappear.
        self.menu = None;
        let tab = &mut self.tabs[index];
        let Some(position) = tab.results.iter().position(|result| result.id == id) else {
            return;
        };
        tab.results.remove(position);
        if tab.live == Some(id) {
            // The session keeps the cursor until the next run closes it.
            tab.live = None;
            tab.more = false;
            tab.pending_page = None;
        }
        if tab.results.is_empty() {
            let table = match self.tabs[index].spare_table.take() {
                Some(table) => table,
                None => self.result_table(self.tabs[index].saved.profile, window, cx),
            };
            self.tabs[index].add_result(table);
        }
        let tab = &mut self.tabs[index];
        if tab.active_result == id {
            tab.active_result = tab.results[position.min(tab.results.len() - 1)].id;
        }
        self.prepare_spare_result(index, window, cx);
        cx.notify();
    }

    fn open_result_rename(
        &mut self,
        tab_id: Uuid,
        id: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() {
            return;
        }
        let Some(current_title) = self
            .tabs
            .iter()
            .find(|tab| tab.saved.id == tab_id)
            .and_then(|tab| tab.result(id))
            .map(|result| result.title.clone())
        else {
            return;
        };
        let title = cx.new(|cx| InputState::new(window, cx).default_value(current_title));
        self.result_form = Some(ResultEditor {
            tab: tab_id,
            result: id,
            title,
            error: None,
        });
        self.open_rename_dialog(RESULT_RENAME, window, cx);
        cx.notify();
    }

    /// Saves the name of a result tab and pins it, so the next run keeps it.
    pub(super) fn rename_result(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = &self.result_form else {
            return;
        };
        let title = form.title.read(cx).value().trim().to_owned();
        let (tab_id, id) = (form.tab, form.result);
        let error = if title.chars().count() > MAX_TAB_TITLE {
            Some(format!(
                "Result name must be {MAX_TAB_TITLE} characters or fewer."
            ))
        } else if !title.is_empty()
            && self
                .tabs
                .iter()
                .find(|tab| tab.saved.id == tab_id)
                .is_some_and(|tab| {
                    tab.results
                        .iter()
                        .any(|result| result.id != id && result.title == title)
                })
        {
            Some("A result with this name already exists in this tab.".into())
        } else {
            None
        };
        if let Some(error) = error {
            if let Some(form) = &mut self.result_form {
                form.error = Some(error);
            }
            cx.notify();
            return;
        }
        if !title.is_empty()
            && let Some(index) = self.tabs.iter().position(|tab| tab.saved.id == tab_id)
        {
            let tab = &mut self.tabs[index];
            let can_pin = tab.pinned_results() < MAX_PINNED_RESULTS;
            if let Some(result) = tab.result_mut(id) {
                result.title = title;
                result.pinned |= can_pin;
            }
            self.prepare_spare_result(index, window, cx);
        }
        self.result_form = None;
        // Programmatic close_dialog does not invoke Dialog::on_close.
        window.close_dialog(cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The summary as one line of all the SQL, cut to the tooltip length.
    fn reference(sql: &str) -> String {
        let line = sql.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.chars().count() <= TOOLTIP_SQL_CHARS {
            return line;
        }
        let mut short: String = line.chars().take(TOOLTIP_SQL_CHARS).collect();
        short.push('…');
        short
    }

    #[::core::prelude::v1::test]
    fn the_sql_summary_is_one_line_cut_to_the_tooltip_length() {
        let word = "x".repeat(TOOLTIP_SQL_CHARS / 2);
        for sql in [
            String::new(),
            "  SELECT\n\t1  ".to_owned(),
            "x".repeat(TOOLTIP_SQL_CHARS),
            "x".repeat(TOOLTIP_SQL_CHARS + 1),
            // The space between the words is the last character that fits.
            format!("{} {}", "x".repeat(TOOLTIP_SQL_CHARS - 1), word),
            format!("{} {}", "x".repeat(TOOLTIP_SQL_CHARS), word),
            format!("SELECT '日本😀'\n  FROM t\n{}", "WHERE a = 1\n".repeat(50)),
        ] {
            assert_eq!(sql_summary(&sql), reference(&sql), "{sql:?}");
        }
    }
}
