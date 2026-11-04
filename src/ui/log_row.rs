//! Shared time, severity, and message lanes for query Logs and Activity.
use super::*;
use crate::logs::timestamp_label;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{h_flex, popover::Popover, v_flex};
use std::time::SystemTime;

#[derive(Clone, PartialEq)]
pub(super) struct Typography {
    pub family: SharedString,
    pub size: Pixels,
    pub line_height: f32,
}

#[derive(IntoElement)]
pub(super) struct LogRow {
    pub id: ElementId,
    pub timestamp: SystemTime,
    pub severity: Severity,
    pub text: String,
    pub order: u64,
    pub typography: Typography,
}

impl RenderOnce for LogRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let timestamp = timestamp_label(self.timestamp);
        let clock = timestamp
            .split_whitespace()
            .last()
            .unwrap_or(&timestamp)
            .to_owned();
        let (first, rest) = self
            .text
            .split_once('\n')
            .map_or((self.text.as_str(), None), |(first, rest)| {
                (first, Some(rest.to_owned()))
            });
        let error = self.severity == Severity::Error;
        h_flex()
            .id(self.id)
            .test_support()
            .w_full()
            .items_start()
            .gap_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border.opacity(0.5))
            .when(error, |el| el.bg(cx.theme().danger.opacity(0.05)))
            .child(
                div()
                    .id("timestamp")
                    .w_20()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(timestamp.clone())
                            .build(window, cx)
                    })
                    .child(clock),
            )
            .child(
                div()
                    .w_12()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(if error {
                        cx.theme().danger
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(if error { "Error" } else { "Info" }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .font_family(self.typography.family.clone())
                    .text_size(self.typography.size)
                    .line_height(relative(self.typography.line_height))
                    .whitespace_normal()
                    .when(error, |el| el.text_color(cx.theme().danger))
                    .child(
                        SelectableText::new("message", first.to_owned()).document_order(self.order),
                    ),
            )
            .when_some(rest, |el, rest| {
                el.child(
                    Popover::new("log-detail")
                        .trigger(
                            Button::new("log-detail-trigger")
                                .ghost()
                                .xsmall()
                                .label("Details…")
                                .accessibility_label("Show log details"),
                        )
                        .content(move |_, _, cx| {
                            div()
                                .id("log-detail-scroll")
                                .w_96()
                                .max_h_80()
                                .overflow_y_scroll()
                                .text_sm()
                                .font_family(cx.theme().mono_font_family.clone())
                                .child(SelectableText::new("details", rest.clone()))
                                .into_any_element()
                        }),
                )
            })
    }
}
