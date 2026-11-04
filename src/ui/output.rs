use super::*;
use gpui_kit::component::{h_flex, v_flex};

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
                    let show_activity =
                        connection.filter(|_| entry.kind == LogKind::KeepAliveFailed);
                    h_flex()
                        .id(("output-entry", entry.id()))
                        .w_full()
                        .items_start()
                        .gap_2()
                        .child(if entry.kind == LogKind::HistoryTrimmed {
                            div()
                                .py_2()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(entry.text.clone())
                                .into_any_element()
                        } else {
                            super::log_row::LogRow {
                                id: ("output-log", entry.id()).into(),
                                timestamp: entry.timestamp,
                                severity: entry.severity,
                                text: entry.text.clone(),
                                order: index as u64 * 2,
                                typography: super::log_row::Typography {
                                    family: self.settings.logs_font_family.clone().into(),
                                    size: self.ui_px(self.settings.logs_font_size),
                                    line_height: self.settings.logs_line_height,
                                },
                            }
                            .into_any_element()
                        })
                        .when_some(show_activity, |el, connection| {
                            el.child(
                                Button::new(("output-show-activity", entry.id()))
                                    .ghost()
                                    .xsmall()
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

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(
                h_flex()
                    .h_10()
                    .flex_shrink_0()
                    .px_3()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{} entries", tab.output.entries().len())),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("output-copy-all")
                            .ghost()
                            .small()
                            .label("Copy all")
                            .disabled(tab.output.is_empty())
                            .accessibility_label("Copy all logs")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_output(cx))),
                    )
                    .child(
                        Button::new("output-copy-error")
                            .ghost()
                            .small()
                            .label("Copy error")
                            .disabled(!tab.output.has_error())
                            .accessibility_label("Copy latest error")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_output_error(cx))),
                    )
                    .child(
                        Button::new("output-clear")
                            .ghost()
                            .small()
                            .label("Clear")
                            .accessibility_label("Clear logs history")
                            .on_click(cx.listener(|this, _, _, cx| this.clear_output(cx))),
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
