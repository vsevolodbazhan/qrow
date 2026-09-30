//! The rows of the conversation transcript.
use super::*;

impl Qrow {
    pub(super) fn assistant_tool_entry(
        &self,
        thread: &str,
        index: usize,
        entry: &TranscriptEntry,
        tool: &ToolActivity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let destructive = cx.theme().semantic_tokens().colors.destructive;
        let accessible_label = entry.label().clone();
        let summary = h_flex()
            .flex_1()
            .min_w_0()
            .gap_1p5()
            .text_color(muted)
            .map(|row| match tool.state {
                ToolState::Running(_) => row.child(Spinner::new().xsmall().color(muted)),
                ToolState::Failed => row.child(
                    Icon::new(IconName::CircleX)
                        .xsmall()
                        .text_color(destructive),
                ),
                _ => row.child(Icon::new(tool.kind.icon()).xsmall()),
            })
            .child(
                div()
                    .flex_none()
                    .font_weight(FontWeight::MEDIUM)
                    .child(tool.kind.title()),
            )
            .child(div().flex_1())
            .when_some(tool.state.label().map(str::to_owned), |row, state| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .when(tool.state == ToolState::Failed, |label| {
                            label.text_color(destructive)
                        })
                        .child(state),
                )
            });
        let header = if entry.detail.is_some() {
            let id = entry.id;
            let thread = thread.to_owned();
            Button::new(format!("assistant-tool-{id}"))
                .ghost()
                .small()
                .w_full()
                .h_auto()
                .min_h_7()
                .py_1()
                .rounded(ButtonRounded::None)
                .toggled(entry.expanded)
                .accessibility_label(accessible_label)
                .child(
                    summary.child(
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(muted)
                            .rotate(percentage(if entry.expanded { 0.25 } else { 0. })),
                    ),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(entry) = this
                        .assistant_panel
                        .transcripts
                        .get_mut(&thread)
                        .and_then(|entries| entries.iter_mut().find(|entry| entry.id == id))
                    {
                        entry.expanded = !entry.expanded;
                        cx.notify();
                    }
                }))
                .into_any_element()
        } else {
            h_flex()
                .id(format!("assistant-tool-{}", entry.id))
                .role(Role::Paragraph)
                .aria_label(accessible_label)
                .min_h_7()
                .py_1()
                .px_2()
                .text_sm()
                .child(summary)
                .into_any_element()
        };
        Collapsible::new()
            .open(entry.expanded)
            .flex_shrink_0()
            .w_full()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius_lg)
            .overflow_hidden()
            .child(header)
            .when_some(entry.detail.as_ref(), |card, detail| {
                card.content(
                    v_flex()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .when_some(tool.target.clone(), |content, target| {
                            content.child(
                                h_flex()
                                    .id(format!("assistant-tool-tab-{}", entry.id))
                                    .test_support()
                                    .role(Role::Paragraph)
                                    .aria_label(format!("Tab: {target}"))
                                    .px_2()
                                    .pt_1p5()
                                    .gap_1p5()
                                    .text_xs()
                                    .child(div().flex_none().text_color(muted).child("Tab"))
                                    .child(div().min_w_0().text_ellipsis().child(target)),
                            )
                        })
                        .child(
                            div()
                                .id(format!("assistant-tool-detail-{}", entry.id))
                                .test_support()
                                .role(Role::Paragraph)
                                .aria_label(detail.clone())
                                .max_h_40()
                                .overflow_y_scroll()
                                .overflow_x_scroll()
                                .px_2()
                                .py_1p5()
                                .font_family(self.settings.editor_font_family.clone())
                                .text_xs()
                                .child(
                                    SelectableText::new("detail", detail.clone())
                                        .document_order(index as u64 * 2 + 1),
                                ),
                        ),
                )
            })
            .into_any_element()
    }
}
