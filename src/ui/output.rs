use super::*;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{Selectable, h_flex, v_flex};
use std::time::{SystemTime, UNIX_EPOCH};

fn timestamp_label(timestamp: SystemTime) -> String {
    let seconds = timestamp
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;

    // Convert days since 1970-01-01 to a Gregorian date without another crate.
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }).div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_part = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096)
        .div_euclid(365);
    let year = year_part + era * 400;
    let day_of_year = day_of_era - (365 * year_part + year_part / 4 - year_part / 100);
    let month_part = (5 * day_of_year + 2).div_euclid(153);
    let day = day_of_year - (153 * month_part + 2).div_euclid(5) + 1;
    let month = month_part + if month_part < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

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
        let all_text = tab.output.copy_all();
        let error_text = tab.output.copy_error();
        let entries: Vec<_> = tab.output.entries().collect();
        let content = if entries.is_empty() {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(cx.theme().muted_foreground)
                .child("No activity yet")
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .children(entries.into_iter().enumerate().map(|(index, entry)| {
                    let (first_line, remaining_lines) = entry
                        .text
                        .split_once('\n')
                        .map_or((entry.text.as_str(), None), |(first, rest)| {
                            (first, Some(rest))
                        });
                    v_flex()
                        .id(("output-entry", entry.id()))
                        .w_full()
                        .font_family(self.settings.logs_font_family.clone())
                        .text_size(self.ui_px(self.settings.logs_font_size))
                        .line_height(relative(self.settings.logs_line_height))
                        .whitespace_normal()
                        .when(entry.severity == Severity::Error, |el| {
                            el.text_color(cx.theme().danger)
                        })
                        .child(
                            SelectableText::new(
                                "header",
                                format!("[{}] {first_line}", timestamp_label(entry.timestamp)),
                            )
                            .document_order(index as u64 * 2),
                        )
                        .when_some(remaining_lines, |el, text| {
                            el.child(
                                SelectableText::new("body", text.to_owned())
                                    .document_order(index as u64 * 2 + 1),
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
                            .child(format!("{} entries", tab.output.entries().count())),
                    )
                    .when(tab.output.retention_notice(), |el| {
                        el.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().warning)
                                .child("Older activity was removed"),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        Button::new("output-copy-all")
                            .ghost()
                            .small()
                            .label("Copy All")
                            .disabled(all_text.is_empty())
                            .accessibility_label("Copy All Logs")
                            .on_click(cx.listener(|this, _, _, cx| this.copy_output(cx))),
                    )
                    .child(
                        Button::new("output-copy-error")
                            .ghost()
                            .small()
                            .label("Copy Error")
                            .disabled(error_text.is_none())
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
