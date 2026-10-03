use super::*;
use crate::logs::timestamp_label;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{Selectable, h_flex, v_flex};

impl Qrow {
    pub(super) fn panel_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.tabs[self.active].panel.selected;
        button_pair::button_pair(
            "panel-switcher",
            Button::new("output-panel-tab")
                .ghost()
                .small()
                .label("Logs")
                .selected(selected == Panel::Output)
                .toggled(selected == Panel::Output)
                .accessibility_label("Logs Panel")
                .on_click(cx.listener(|this, _, _, cx| this.select_panel(Panel::Output, cx))),
            Button::new("results-panel-tab")
                .ghost()
                .small()
                .label("Results")
                .selected(selected == Panel::Results)
                .toggled(selected == Panel::Results)
                .accessibility_label("Results Panel")
                .on_click(cx.listener(|this, _, _, cx| this.select_panel(Panel::Results, cx))),
            cx,
        )
    }

    pub(super) fn output_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = &self.tabs[self.active];
        let connection = tab.worker_profile.or(tab.saved.profile);
        let content = if tab.output.is_empty() {
            panel_empty_state("No logs yet", cx).into_any_element()
        } else {
            v_flex()
                .w_full()
                .children(tab.output.entries().enumerate().map(|(index, entry)| {
                    let (first_line, remaining_lines) = entry
                        .text
                        .split_once('\n')
                        .map_or((entry.text.as_str(), None), |(first, rest)| {
                            (first, Some(rest))
                        });
                    let header = if entry.kind == LogKind::HistoryTrimmed {
                        first_line.to_owned()
                    } else {
                        format!("[{}] {first_line}", timestamp_label(entry.timestamp))
                    };
                    // A failed keep-alive closed the session. Activity shows
                    // what else happened on the connection.
                    let show_activity =
                        connection.filter(|_| entry.kind == LogKind::KeepAliveFailed);
                    let text = v_flex()
                        .flex_1()
                        .min_w_0()
                        .font_family(self.settings.logs_font_family.clone())
                        .text_size(self.ui_px(self.settings.logs_font_size))
                        .line_height(relative(self.settings.logs_line_height))
                        .whitespace_normal()
                        .when(entry.severity == Severity::Error, |el| {
                            el.text_color(cx.theme().danger)
                        })
                        .child(
                            SelectableText::new("header", header).document_order(index as u64 * 2),
                        )
                        .when_some(remaining_lines, |el, text| {
                            el.child(
                                SelectableText::new("body", text.to_owned())
                                    .document_order(index as u64 * 2 + 1),
                            )
                        });
                    h_flex()
                        .id(("output-entry", entry.id()))
                        .w_full()
                        .items_start()
                        .gap_2()
                        .child(text)
                        .when_some(show_activity, |el, connection| {
                            el.child(
                                Button::new(("output-show-activity", entry.id()))
                                    .ghost()
                                    .xsmall()
                                    .label("Show Activity")
                                    .accessibility_label("Show Activity")
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
                    .child(self.panel_switcher(cx))
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
                            .label("Copy All")
                            .disabled(tab.output.is_empty())
                            .accessibility_label("Copy All Logs")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_output(cx))),
                    )
                    .child(
                        Button::new("output-copy-error")
                            .ghost()
                            .small()
                            .label("Copy Error")
                            .disabled(!tab.output.has_error())
                            .accessibility_label("Copy Latest Error")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_output_error(cx))),
                    )
                    .child(
                        Button::new("output-clear")
                            .ghost()
                            .small()
                            .label("Clear")
                            .accessibility_label("Clear Logs History")
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
