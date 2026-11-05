use super::log_row::{self, LogRow};
use super::result_toolbar::ResultSummary;
use super::*;
use gpui_kit::component::{Icon, h_flex, v_flex};

impl Qrow {
    pub(super) fn output_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = &self.tabs[self.active];
        let content = if tab.output.is_empty() {
            panel_empty_state("No logs yet", cx).into_any_element()
        } else {
            list(
                tab.output_list.clone(),
                cx.processor(|this, index, _, cx| this.output_row(index, cx)),
            )
            .size_full()
            .into_any_element()
        };

        let entries = match tab.output.entries().len() {
            1 => "1 entry".to_owned(),
            count => format!("{} entries", crate::logs::group_digits(count)),
        };
        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(
                h_flex()
                    .id("output-toolbar")
                    .h_10()
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    // The same chip and icon buttons as the Result toolbar.
                    .child(
                        div()
                            .id("output-count")
                            .test_support()
                            .role(Role::Label)
                            .whitespace_nowrap()
                            .aria_label(entries.clone())
                            .child(ResultSummary::neutral_tag(cx).child(entries)),
                    )
                    .child(div().flex_1())
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap_1()
                            .child(
                                Button::new("output-copy-all")
                                    .ghost()
                                    .small()
                                    .icon(IconName::Copy)
                                    .tooltip("Copy all logs")
                                    .disabled(tab.output.is_empty())
                                    .accessibility_label("Copy all logs")
                                    .on_click(cx.listener(|this, _, _, cx| this.copy_output(cx))),
                            )
                            // Hidden, not disabled, when no entry is an error.
                            .when(tab.output.has_error(), |el| {
                                el.child(
                                    Button::new("output-copy-error")
                                        .ghost()
                                        .small()
                                        .icon(IconName::CircleX)
                                        .tooltip("Copy latest error")
                                        .accessibility_label("Copy latest error")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| {
                                                this.copy_output_error(cx)
                                            }),
                                        ),
                                )
                            })
                            .child(
                                Button::new("output-clear")
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                                    .tooltip("Clear logs")
                                    .accessibility_label("Clear logs history")
                                    .on_click(cx.listener(|this, _, _, cx| this.clear_output(cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .id("output-scroll")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .px_3()
                    .py_2()
                    .child(content),
            )
            .into_any_element()
    }

    /// The row of the entry at `index` of the Logs of the current tab.
    fn output_row(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let tab = &self.tabs[self.active];
        let Some(entry) = tab.output.entries().nth(index) else {
            return div().into_any_element();
        };
        // A failed keep-alive closed the session. Activity shows what else
        // happened on the connection.
        let show_activity = tab
            .worker_profile
            .or(tab.saved.profile)
            .filter(|_| entry.kind == LogKind::KeepAliveFailed);
        let id = entry.id();
        let long = log_row::has_long_code_block(&entry.text);
        let expanded = tab.expanded_output.contains(&id);
        // GPUI's list lays a row out at the width of its content.
        div()
            .w_full()
            .min_w_0()
            .child(
                LogRow::new(
                    ("output-entry", entry.id()),
                    index as u64,
                    entry.text.clone(),
                )
                .when(entry.kind != LogKind::HistoryTrimmed, |row| {
                    row.time(entry.timestamp)
                })
                .error(entry.severity == Severity::Error)
                .typography(
                    self.settings.logs_font_family.clone().into(),
                    self.ui_px(self.settings.logs_font_size),
                    self.settings.logs_line_height,
                )
                .when_some(show_activity, |row, connection| {
                    row.action(
                        Button::new(("output-show-activity", entry.id()))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(gpui_kit::assets::IconName::Activity))
                            .label("Show activity")
                            .accessibility_label("Show activity")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_activity(Some(connection), window, cx)
                            })),
                    )
                })
                // A submitted query copies its SQL, like Copy query of a result.
                .when_some(
                    log_row::code_block(&entry.text)
                        .filter(|_| entry.kind == LogKind::Submitted)
                        .map(str::to_owned),
                    |row, sql| {
                        row.action(
                            Button::new(("output-copy-query", entry.id()))
                                .ghost()
                                .xsmall()
                                .icon(Icon::new(gpui_kit::assets::IconName::FileCode))
                                .tooltip("Copy query")
                                .accessibility_label("Copy query")
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(sql.clone()))
                                })),
                        )
                    },
                )
                // A long code block shows its first lines until it expands.
                .when(long, |row| {
                    let label = if expanded {
                        "Collapse query"
                    } else {
                        "Expand query"
                    };
                    row.collapsed(!expanded).action(
                        Button::new(("output-expand", id))
                            .ghost()
                            .xsmall()
                            .icon(if expanded {
                                IconName::ChevronUp
                            } else {
                                IconName::ChevronDown
                            })
                            .tooltip(label)
                            .accessibility_label(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_output_entry(id, index, cx)
                            })),
                    )
                }),
            )
            .into_any_element()
    }

    /// Expand or collapse the code block of the Logs entry `id` at `index`.
    fn toggle_output_entry(&mut self, id: u64, index: usize, cx: &mut Context<Self>) {
        let tab = &mut self.tabs[self.active];
        if !tab.expanded_output.remove(&id) {
            tab.expanded_output.insert(id);
        }
        tab.output_list.remeasure_items(index..index + 1);
        cx.notify();
    }

    /// Measure the Logs rows again when their font, size, or line height
    /// changes, also with the interface scale.
    pub(super) fn sync_output_typography(&mut self) {
        let typography = (
            SharedString::from(self.settings.logs_font_family.clone()),
            self.ui_px(self.settings.logs_font_size),
            self.settings.logs_line_height,
        );
        if self.output_typography.as_ref() != Some(&typography) {
            self.output_typography = Some(typography);
            for tab in &self.tabs {
                tab.output_list.remeasure();
            }
        }
    }
}

/// The empty list of the Logs of a new tab. It follows new entries while it
/// shows the end.
pub(super) fn output_list() -> ListState {
    let list = ListState::new(0, ListAlignment::Top, px(OUTPUT_OVERDRAW)).measure_all();
    list.set_follow_mode(FollowMode::Tail);
    list
}

/// Rows that a list draws past each edge of the Logs, so that a selection
/// keeps the rows next to the visible ones.
const OUTPUT_OVERDRAW: f32 = 600.;

/// Change the rows of `list` from the entries `before` to the entries
/// `after`, by entry ID.
pub(super) fn splice_rows(list: &ListState, before: &[u64], after: &[u64]) {
    for (range, count) in row_splices(before, after) {
        list.splice(range, count);
    }
}

/// The splices, in order, that change rows `before` to rows `after`. Entries
/// keep their order, so a walk over both finds the removed and the added
/// runs. Each range is in the rows after the earlier splices.
fn row_splices(before: &[u64], after: &[u64]) -> Vec<(std::ops::Range<usize>, usize)> {
    let old: std::collections::HashSet<u64> = before.iter().copied().collect();
    let new: std::collections::HashSet<u64> = after.iter().copied().collect();
    let (mut i, mut j, mut row) = (0, 0, 0);
    let mut splices = Vec::new();
    while i < before.len() || j < after.len() {
        if i < before.len() && j < after.len() && before[i] == after[j] {
            (i, j, row) = (i + 1, j + 1, row + 1);
        } else if i < before.len() && (!new.contains(&before[i]) || j == after.len()) {
            let removed = before[i..]
                .iter()
                .take_while(|id| !new.contains(id))
                .count()
                .max(1);
            splices.push((row..row + removed, 0));
            i += removed;
        } else if !old.contains(&after[j]) {
            let added = after[j..].iter().take_while(|id| !old.contains(id)).count();
            splices.push((row..row, added));
            (j, row) = (j + added, row + added);
        } else {
            // Not in order: drop the old row and keep walking.
            splices.push((row..row + 1, 0));
            i += 1;
        }
    }
    splices
}

#[cfg(test)]
mod tests {
    use super::row_splices;

    /// Applies `splices` to `rows` the way a list does.
    fn apply(mut rows: Vec<u64>, splices: &[(std::ops::Range<usize>, usize)], after: &[u64]) {
        let original = rows.clone();
        let mut added = after
            .iter()
            .filter(move |id| !original.contains(id))
            .copied();
        for (range, count) in splices {
            let new: Vec<u64> = added.by_ref().take(*count).collect();
            rows.splice(range.clone(), new);
        }
        assert_eq!(rows, after);
    }

    #[test]
    fn splices_follow_appends_and_removed_groups() {
        for (before, after) in [
            (vec![], vec![0]),
            (vec![0, 1], vec![0, 1, 2]),
            // An old group goes from the middle, and a new entry comes last.
            (vec![0, 1, 2, 3], vec![0, 2, 4]),
            // The first groups go, and a retention line comes first.
            (vec![0, 1, 2, 3], vec![9, 2, 3, 4]),
            (vec![0, 1], vec![]),
        ] {
            apply(before.clone(), &row_splices(&before, &after), &after);
        }
    }
}
