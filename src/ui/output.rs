use super::*;
use super::log_row::LogRow;
use super::result_toolbar::ResultSummary;
use gpui_kit::component::{Icon, h_flex, v_flex};

impl Qrow {
    pub(super) fn output_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = &self.tabs[self.active];
        let connection = tab.worker_profile.or(tab.saved.profile);
        let content = if tab.output.is_empty() {
            panel_empty_state("No logs yet", cx).into_any_element()
        } else {
            v_flex()
                .w_full()
                .children(tab.output.entries().enumerate().map(|(index, entry)| {
                    // A failed keep-alive closed the session. Activity shows
                    // what else happened on the connection.
                    let show_activity =
                        connection.filter(|_| entry.kind == LogKind::KeepAliveFailed);
                    LogRow::new(("output-entry", entry.id()), index as u64, entry.text.clone())
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
                }))
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
                                            cx.listener(|this, _, _, cx| this.copy_output_error(cx)),
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
                    .overflow_y_scroll()
                    .track_scroll(&tab.output_scroll)
                    .px_3()
                    .py_2()
                    .child(content),
            )
            .into_any_element()
    }
}
