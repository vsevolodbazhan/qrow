//! The rows of the conversation transcript.
use super::*;

/// One row of the transcript list. `index` is the place of the row, which
/// orders the text selection.
pub(super) fn transcript_row(
    qrow: &Qrow,
    handle: &WeakEntity<Qrow>,
    row: Row,
    index: usize,
    cx: &App,
) -> AnyElement {
    let shown = qrow.shown_conversation();
    match row {
        Row::Older => {
            let run = shown.run;
            Button::new("assistant-load-older")
                .ghost()
                .small()
                .label("Load older messages")
                .disabled(run.is_some_and(|run| {
                    run.loading_older || run.active_turn.is_some() || run.pending_query.is_some()
                }))
                .on_click(on_qrow(handle, |this, _, _, cx| {
                    this.load_older_assistant_messages(cx)
                }))
                .into_any_element()
        }
        Row::Working => Message::new()
            .flex_shrink_0()
            .alignment(MessageAlignment::Start)
            .content(
                MessageContent::new().bubble(
                    Bubble::new().with_variant(BubbleVariant::Ghost).child(
                        div()
                            .id("assistant-working")
                            .test_support()
                            .role(Role::Status)
                            .aria_label("Assistant is working")
                            .child(
                                ShimmerText::new("Working…")
                                    .text_color(cx.theme().muted_foreground),
                            ),
                    ),
                ),
            )
            .into_any_element(),
        Row::Entry { id, .. } => {
            // The rows before the entries hold at most the button.
            let offset = usize::from(
                shown
                    .thread
                    .as_ref()
                    .is_some_and(|thread| qrow.assistant_state.older_cursors.contains_key(thread)),
            );
            let Some(entry) = shown.entry(id, index.saturating_sub(offset)) else {
                return div().into_any_element();
            };
            let thread = shown.thread.as_deref().unwrap_or("");
            match &entry.tool {
                Some(tool) => tool_entry(qrow, handle, thread, index, entry, tool, cx),
                None => message_entry(qrow, entry, cx),
            }
        }
    }
}

/// A message, a reply, or an error.
fn message_entry(qrow: &Qrow, entry: &TranscriptEntry, cx: &App) -> AnyElement {
    let mut table_style = StyleRefinement::default();
    table_style.overflow.x = Some(Overflow::Scroll);
    let content = TextView::markdown(
        format!("assistant-markdown-{}", entry.id),
        entry.text().clone(),
    )
    .style(TextViewStyle::default().table(table_style))
    .font_family(qrow.settings.assistant_font_family.clone())
    .text_size(qrow.ui_px(qrow.settings.assistant_font_size))
    .line_height(relative(qrow.settings.assistant_line_height))
    .min_w_0()
    .max_w_full()
    .when(entry.speaker == Speaker::Error, |view| {
        view.text_color(cx.theme().semantic_tokens().colors.destructive)
    });
    let alignment = if entry.speaker == Speaker::User {
        MessageAlignment::End
    } else {
        MessageAlignment::Start
    };
    let variant = match entry.speaker {
        Speaker::User => BubbleVariant::Tinted,
        Speaker::Assistant => BubbleVariant::Ghost,
        Speaker::Activity => BubbleVariant::Outline,
        Speaker::Error => BubbleVariant::Destructive,
    };
    // Give every level a definite width before Markdown measures its wrapped
    // height. A shrink-to-fit bubble measures a table at one width and draws
    // it at another, so the transcript clips it. Replies use the full width,
    // as tool cards do.
    let full_width = entry.speaker != Speaker::User;
    Message::new()
        .flex_shrink_0()
        .alignment(alignment)
        .content(
            MessageContent::new()
                .map(|content| {
                    if full_width {
                        content.w_full()
                    } else {
                        content.w(relative(0.8))
                    }
                })
                .bubble(
                    Bubble::new()
                        .with_variant(variant)
                        .max_w_full()
                        .when(full_width, |bubble| bubble.w_full())
                        .content(
                            BubbleContent::new()
                                .text_base()
                                .when(full_width, |content| content.w_full()),
                        )
                        .child(
                            div()
                                .id(format!("assistant-entry-{}", entry.id))
                                .test_support()
                                .role(Role::Paragraph)
                                .aria_label(entry.label().clone())
                                .whitespace_normal()
                                .child(content),
                        ),
                ),
        )
        .into_any_element()
}

/// A collapsed card of a Qrow tool call. A card with details opens.
fn tool_entry(
    qrow: &Qrow,
    handle: &WeakEntity<Qrow>,
    thread: &str,
    index: usize,
    entry: &TranscriptEntry,
    tool: &ToolActivity,
    cx: &App,
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
        .when_some(
            tool.state.label().map(|label| label.into_owned()),
            |row, state| {
                row.child(
                    div()
                        .id(SharedString::from(format!(
                            "assistant-tool-outcome-{}",
                            entry.id
                        )))
                        .test_support()
                        .aria_label(state.clone())
                        .flex_none()
                        .text_xs()
                        .when(tool.state == ToolState::Failed, |label| {
                            label.text_color(destructive)
                        })
                        .child(state),
                )
            },
        )
        .when_some(tool.state.elapsed(), |row, elapsed| {
            row.child(
                div()
                    .id(SharedString::from(format!(
                        "assistant-tool-elapsed-{}",
                        entry.id
                    )))
                    .test_support()
                    .aria_label(format!("Elapsed: {:.2} s", elapsed.as_secs_f64()))
                    .flex_none()
                    .text_xs()
                    .child(format!("{:.2} s", elapsed.as_secs_f64())),
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
            .toggled(entry.expanded())
            .accessibility_label(accessible_label)
            .child(
                summary.child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted)
                        .rotate(percentage(if entry.expanded() { 0.25 } else { 0. })),
                ),
            )
            .on_click(on_qrow(handle, move |this, _, _, cx| {
                if let Some(entry) = this
                    .assistant_state
                    .transcripts
                    .get_mut(&thread)
                    .and_then(|entries| entries.iter_mut().find(|entry| entry.id == id))
                {
                    entry.toggle_expanded();
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
        .open(entry.expanded())
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
                            .font_family(qrow.settings.editor_font_family.clone())
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
