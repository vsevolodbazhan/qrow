//! The assistant pane: header, transcript, approval card, and message field.
use super::*;

pub(super) fn show_thread_list(narrow: bool, override_visibility: Option<bool>) -> bool {
    override_visibility.unwrap_or(!narrow)
}

/// A narrow pane swaps the opened list for the selected conversation. A wide
/// pane keeps the list next to it.
pub(super) fn thread_list_after_selection(override_visibility: Option<bool>) -> Option<bool> {
    match override_visibility {
        Some(true) => None,
        other => other,
    }
}

pub(super) fn assistant_select_width(label: &str) -> f32 {
    56. + label.chars().count() as f32 * 8.
}

pub(super) fn assistant_selector_labels(available: f32, widths: [f32; 3]) -> [bool; 3] {
    let icons = 36. * 3. + 8.;
    let model = available >= icons + widths[0] - 36.;
    let reasoning = model && available >= icons + widths[0] + widths[1] - 72.;
    let tier = reasoning && available >= widths.iter().sum::<f32>() + 8.;
    [model, reasoning, tier]
}

impl Qrow {
    /// The icon of a conversation state. An idle conversation has no icon.
    pub(in crate::ui) fn assistant_status_icon(
        &self,
        status: ThreadStatus,
        cx: &App,
    ) -> Option<AnyElement> {
        let theme = cx.theme();
        Some(match status {
            ThreadStatus::Idle => return None,
            ThreadStatus::Working => Spinner::new()
                .xsmall()
                .color(theme.primary)
                .into_any_element(),
            ThreadStatus::Approval => Icon::new(AssetIconName::Bot)
                .small()
                .text_color(theme.warning)
                .into_any_element(),
            ThreadStatus::Ready => Icon::new(AssetIconName::Bot)
                .small()
                .text_color(theme.success)
                .into_any_element(),
            ThreadStatus::Failed => Icon::new(AssetIconName::TriangleAlert)
                .small()
                .text_color(theme.danger)
                .into_any_element(),
        })
    }

    pub(in crate::ui) fn assistant_panel(
        &self,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let narrow = width < self.ui_px(600.);
        let show_threads = show_thread_list(narrow, self.assistant_panel.thread_list_override);
        let action_size = self.ui_px(28.);
        let controls_ready = matches!(self.assistant_panel.status, Status::Ready);
        let disconnected_error = match &self.assistant_panel.status {
            Status::Disconnected(error) => Some(error.clone()),
            _ => None,
        };
        let disconnected = disconnected_error.is_some();
        let signed_out = matches!(self.assistant_panel.status, Status::SignInRequired);
        let displayed = self.displayed_thread();
        let selected = displayed.as_deref().unwrap_or("");
        let run = displayed
            .as_deref()
            .and_then(|thread| self.thread_run(thread));
        let active_turn = run.is_some_and(|run| run.active_turn.is_some());
        let active_tab = self.active_tab_id();
        let first_message = self.assistant_panel.first_messages.iter().find(|first| {
            self.assistant_panel.browsed_thread.is_none() && Some(first.tab_id) == active_tab
        });
        let pending_approval = run
            .and_then(|run| run.pending_query.as_ref())
            .filter(|pending| !pending.approved);
        // Codex owes a reply from the send until the turn ends. A query that
        // waits for approval waits for you instead.
        let waiting_for_agent = first_message.is_some()
            || (run.is_some_and(|run| run.pending_reply || run.active_turn.is_some())
                && pending_approval.is_none());
        let entries = self.assistant_panel.transcripts.get(selected);
        let busy = displayed
            .as_deref()
            .is_some_and(|thread| self.thread_status(thread).busy());
        let mode_is_run = self.displayed_mode() == AssistantExecutionMode::RunAutomatically;
        let model = self.assistant_panel.snapshot.as_ref().and_then(|snapshot| {
            self.settings
                .assistant
                .model
                .as_deref()
                .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
                .or_else(|| snapshot.models().first())
        });
        let model_label = model
            .map(|model| model.display_name().to_owned())
            .unwrap_or_default();
        let reasoning_label = model
            .and_then(|model| {
                let efforts = model.reasoning_efforts();
                let find = |id: &str| efforts.iter().find(|effort| effort.id() == id);
                self.settings
                    .assistant
                    .reasoning_effort
                    .as_deref()
                    .and_then(find)
                    .or_else(|| find(model.default_reasoning_effort()))
                    .or_else(|| efforts.first())
            })
            .map(|effort| reasoning_effort_label(effort.id()))
            .unwrap_or_default();
        let tier_label = model
            .map(|model| {
                self.settings
                    .assistant
                    .service_tier
                    .as_deref()
                    .and_then(|id| model.service_tiers().iter().find(|tier| tier.id() == id))
                    .map_or("Default", |tier| tier.name())
                    .to_owned()
            })
            .unwrap_or_default();
        // Names a composer control with its value. Codex supplies the values,
        // so the controls wait for Codex before they show one.
        let control_label = |name: &str, value: &str| {
            if controls_ready && model.is_some() {
                format!("{name}: {value}")
            } else {
                format!("{name}: waiting for Codex")
            }
        };
        let model_width = assistant_select_width(&model_label);
        let reasoning_width = assistant_select_width(&reasoning_label);
        let tier_width = assistant_select_width(&tier_label);
        let model_options = self
            .assistant_panel
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .models()
                    .iter()
                    .map(|candidate| {
                        (
                            candidate.display_name().to_owned(),
                            self.settings.assistant.model.as_deref() == Some(candidate.id()),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let reasoning_options = model
            .map(|model| {
                model
                    .reasoning_efforts()
                    .iter()
                    .map(|effort| {
                        let label = reasoning_effort_label(effort.id());
                        let selected = label == reasoning_label;
                        (label, selected)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let tier_options = model
            .map(|model| {
                let mut options = vec![(
                    "Default".to_owned(),
                    self.settings.assistant.service_tier.is_none(),
                )];
                options.extend(model.service_tiers().iter().map(|tier| {
                    (
                        tier.name().to_owned(),
                        self.settings.assistant.service_tier.as_deref() == Some(tier.id()),
                    )
                }));
                options
            })
            .unwrap_or_default();
        let assistant_entity = cx.entity();
        let conversation_width = width.as_f32() / self.settings.ui_scale
            - if show_threads && !narrow { 230. } else { 0. };
        let [show_model_label, show_reasoning_label, show_tier_label] = assistant_selector_labels(
            (conversation_width - 196.).max(0.),
            [model_width, reasoning_width, tier_width],
        );
        let displayed_title = displayed
            .as_deref()
            .and_then(|thread| self.assistant.conversation(thread))
            .map_or_else(
                || "New conversation".to_owned(),
                |conversation| conversation.title.clone(),
            );
        let title_generating = self.assistant_title_generating(selected);
        h_flex()
            .size_full()
            .min_w_0()
            .bg(cx.theme().sidebar)
            .when(!narrow || !show_threads, |panel| panel.child(v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                h_flex()
                    .h(self.ui_px(36.))
                    .flex_shrink_0()
                    .items_center()
                    .pl_3()
                    .pr_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .id("assistant-conversation-title")
                            .test_support()
                            .role(Role::Status)
                            .aria_label(if title_generating {
                                format!("Generating title for {displayed_title}")
                            } else {
                                format!("Conversation title: {displayed_title}")
                            })
                            .when(title_generating, |title| {
                                title.child(
                                    ShimmerText::new(displayed_title.clone())
                                        .id(format!("assistant-title-header-{selected}")),
                                )
                            })
                            .when(!title_generating, |title| title.child(displayed_title)),
                    )
                    .child(
                        Button::new("assistant-new")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .flex_shrink_0()
                            .icon(IconName::Plus)
                            .accessibility_label("New Conversation")
                            .tooltip("New Conversation")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.create_assistant_conversation(window, cx);
                            })),
                    )
                    .child(
                        Button::new("assistant-conversation-menu")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .icon(IconName::Ellipsis)
                            .accessibility_label("Conversation Actions")
                            .tooltip("Conversation Actions")
                            .disabled(displayed.is_none() || busy)
                            .dropdown_menu({
                                let menu = self.conversation_menu(selected, cx);
                                move |popup, _, _| menu.build(popup)
                            }),
                    )
                    .child(
                        Button::new("assistant-toggle-threads")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .flex_shrink_0()
                            .icon(IconName::Menu)
                            .accessibility_label("Toggle Conversation List")
                            .tooltip("Toggle Conversation List")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.assistant_panel.thread_list_override = Some(!show_threads);
                                cx.notify();
                            })),
                    )
            )
            .when_some(self.assistant_panel.notice.as_ref(), |panel, notice| {
                let (alert, message) = match notice {
                    AssistantNotice::Info(message) => {
                        (Alert::info("assistant-notice", message.clone()), message)
                    }
                    AssistantNotice::Warning(message) => {
                        (Alert::warning("assistant-notice", message.clone()), message)
                    }
                    AssistantNotice::Error(message) => {
                        (Alert::error("assistant-notice", message.clone()), message)
                    }
                };
                panel.child(
                    div()
                        .id("assistant-notice-accessibility")
                        .test_support()
                        .role(Role::Alert)
                        .aria_label(message.clone())
                        .px_3()
                        .py_1()
                        .child(alert.small()),
                )
            })
            .when(signed_out, |panel| panel.child(
                div()
                    .id("assistant-sign-in")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.assistant_sign_in(cx))))
            .when(!signed_out, |panel| panel.child(
                div().relative().flex_1().min_h_0().child(v_flex()
                    .id("assistant-transcript")
                    .test_support()
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.assistant_panel.scroll)
                    .px_3()
                    .py_3()
                    .gap_3()
                    .when(self.assistant_panel.older_cursors.contains_key(selected), |transcript| transcript.child(
                        Button::new("assistant-load-older").ghost().small().label("Load older messages")
                            .disabled(run.is_some_and(|run| run.loading_older || run.active_turn.is_some() || run.pending_query.is_some()))
                            .on_click(cx.listener(|this, _, _, cx| this.load_older_assistant_messages(cx)))))
                    .children(
                        entries
                            .into_iter()
                            .flatten()
                            .chain(first_message.map(|first| &first.entry))
                            .enumerate()
                            .map(|(index, entry)| {
                                if let Some(tool) = &entry.tool {
                                    return self.assistant_tool_entry(selected, index, entry, tool, cx);
                                }
                                let mut table_style = StyleRefinement::default();
                                table_style.overflow.x = Some(Overflow::Scroll);
                                let content = TextView::markdown(
                                    format!("assistant-markdown-{}", entry.id),
                                    entry.text().clone(),
                                )
                                .style(TextViewStyle::default().table(table_style))
                                .font_family(self.settings.assistant_font_family.clone())
                                .text_size(self.ui_px(self.settings.assistant_font_size))
                                .line_height(relative(self.settings.assistant_line_height))
                                .min_w_0()
                                .max_w_full()
                                .when(entry.speaker == Speaker::Error, |view| {
                                    view.text_color(
                                        cx.theme().semantic_tokens().colors.destructive,
                                    )
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
                                // Give every level a definite width before Markdown measures
                                // its wrapped height. A shrink-to-fit bubble measures a table at
                                // one width and draws it at another, so the transcript clips it.
                                // Replies use the full width, as tool cards do.
                                let full_width = entry.speaker != Speaker::User;
                                Message::new().flex_shrink_0().alignment(alignment).content(
                                    MessageContent::new()
                                        .map(|content| if full_width { content.w_full() } else { content.w(relative(0.8)) })
                                        .bubble(
                                        Bubble::new()
                                            .with_variant(variant)
                                            .max_w_full()
                                            .when(full_width, |bubble| bubble.w_full())
                                            .content(BubbleContent::new().text_base().when(full_width, |content| content.w_full()))
                                            .child(
                                        div()
                                            .id(format!("assistant-entry-{}", entry.id))
                                            .test_support()
                                            .role(Role::Paragraph)
                                            .aria_label(entry.label().clone())
                                            .whitespace_normal()
                                            .child(content)),
                                    ),
                                )
                                .into_any_element()
                            }),
                    )
                    .when(waiting_for_agent, |transcript| {
                            transcript.child(
                                Message::new()
                                    .flex_shrink_0()
                                    .alignment(MessageAlignment::Start)
                                    .content(MessageContent::new().bubble(
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
                                    )),
                            )
                    }))
            .when(
                entries.is_some_and(|entries| !entries.is_empty())
                    && !self.assistant_transcript_near_bottom(),
                |transcript| {
                    transcript.child(
                        Button::new("assistant-jump-latest")
                            .small()
                            .absolute()
                            .bottom_2()
                            .right_3()
                            .icon(IconName::ArrowDown)
                            .accessibility_label("Jump to Latest")
                            .tooltip("Jump to Latest")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.assistant_panel.scroll.scroll_to_bottom();
                                cx.notify();
                            })),
                    )
                },
            )))
            .when_some(
                pending_approval,
                |panel, pending| {
                    let tab = self.tabs.iter().find(|tab| tab.saved.id == pending.tab_id);
                    let tab_title = tab.map_or("Query tab", |tab| tab.saved.title.as_str());
                    let connection = tab
                        .and_then(|tab| tab.saved.profile)
                        .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
                        .map_or("No connection", |profile| profile.name.as_str());
                    panel.child(
                        v_flex()
                            .id("assistant-query-approval")
                            .test_support()
                            .role(Role::Alert)
                            .aria_label(format!(
                                "Run in {tab_title} · {connection}? {}",
                                pending.sql
                            ))
                            .p_3()
                            .gap_2()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("Run in {tab_title} · {connection}?")),
                            )
                            .child(
                                div()
                                    .id("assistant-approval-sql")
                                    .font_family(self.settings.editor_font_family.clone())
                                    .text_xs()
                                    .max_h_32()
                                    .overflow_y_scroll()
                                    .child(pending.sql.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Qrow cannot confirm that this SQL is read-only."),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new("assistant-approve-query")
                                            .primary()
                                            .small()
                                            .label("Run")
                                            .on_click(cx.listener({
                                                let thread = selected.to_owned();
                                                move |this, _, window, cx| {
                                                    this.approve_assistant_query(&thread, window, cx)
                                                }
                                            })),
                                    )
                                    .child(
                                        Button::new("assistant-cancel-query")
                                            .small()
                                            .label("Cancel")
                                            .on_click(cx.listener({
                                                let thread = selected.to_owned();
                                                move |this, _, _, cx| {
                                                    this.cancel_assistant_approval(&thread, cx)
                                                }
                                            })),
                                    ),
                            ),
                    )
                },
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_3()
                    .gap_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        // GPUI Kit's Textarea has no ID setter; tests find the
                        // message field through its container.
                        div().id("assistant-composer").test_support().key_context("AssistantComposer").child(
                            Textarea::new(&self.assistant_panel.composer)
                                .h_20()
                                .w_full()
                                .aria_label("Assistant Message")),
                    )
                    .child(
                        h_flex().min_w_0().gap_1()
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_1()
                                    .child(
                                        Button::new("assistant-model")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::Cpu)
                                            .accessibility_label(control_label("Model", &model_label))
                                            .tooltip(control_label("Model", &model_label))
                                            .disabled(!controls_ready || model.is_none())
                                            .when(model.is_some() && show_model_label, |button| button.label(model_label.clone()).dropdown_caret(true))
                                            .dropdown_menu({
                                                let assistant_entity = assistant_entity.clone();
                                                move |menu, _, _| model_options.iter().fold(menu, |menu, (label, selected)| {
                                                    let label = label.clone();
                                                    let assistant_entity = assistant_entity.clone();
                                                    menu.item(PopupMenuItem::new(label.clone()).checked(*selected).on_click(
                                                        move |_, _, cx| {
                                                            assistant_entity.update(cx, |this, cx| {
                                                                this.select_assistant_model(&label, cx);
                                                            });
                                                        },
                                                    ))
                                                })
                                            }))
                                    .child(
                                        Button::new("assistant-reasoning")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::Asterisk)
                                            .accessibility_label(control_label("Reasoning", &reasoning_label))
                                            .tooltip(control_label("Reasoning", &reasoning_label))
                                            .disabled(!controls_ready || !model.is_some_and(|model| !model.reasoning_efforts().is_empty()))
                                            .when(model.is_some() && show_reasoning_label, |button| button.label(reasoning_label.clone()).dropdown_caret(true))
                                            .dropdown_menu({
                                                let assistant_entity = assistant_entity.clone();
                                                move |menu, _, _| reasoning_options.iter().fold(menu, |menu, (label, selected)| {
                                                    let label = label.clone();
                                                    let assistant_entity = assistant_entity.clone();
                                                    menu.item(PopupMenuItem::new(label.clone()).checked(*selected).on_click(
                                                        move |_, _, cx| {
                                                            assistant_entity.update(cx, |this, cx| {
                                                                this.select_assistant_reasoning(&label, cx);
                                                            });
                                                        },
                                                    ))
                                                })
                                            }))
                                    .child(
                                        Button::new("assistant-tier")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::BatteryCharging)
                                            .accessibility_label(control_label("Service tier", &tier_label))
                                            .tooltip(control_label("Service tier", &tier_label))
                                            .disabled(!controls_ready || !model.is_some_and(|model| !model.service_tiers().is_empty()))
                                            .when(model.is_some() && show_tier_label, |button| button.label(tier_label.clone()).dropdown_caret(true))
                                            .dropdown_menu({
                                                let assistant_entity = assistant_entity.clone();
                                                move |menu, _, _| tier_options.iter().fold(menu, |menu, (label, selected)| {
                                                    let label = label.clone();
                                                    let assistant_entity = assistant_entity.clone();
                                                    menu.item(PopupMenuItem::new(label.clone()).checked(*selected).on_click(
                                                        move |_, _, cx| {
                                                            assistant_entity.update(cx, |this, cx| {
                                                                this.select_assistant_tier(&label, cx);
                                                            });
                                                        },
                                                    ))
                                                })
                                            })),
                            )
                            .child(div().flex_1())
                            .when_some(disconnected_error, |row, error| row.child(
                                Button::new("assistant-reconnect")
                                    .small()
                                    .icon(AssetIconName::RotateCw)
                                    .label("Reconnect")
                                    .accessibility_label("Reconnect to Codex")
                                    .tooltip(error)
                                    .on_click(cx.listener(|this, _, window, cx| this.reconnect_assistant(window, cx))),
                            ))
                            .when(!disconnected && active_turn, |row| row.child(
                                Button::new("assistant-stop")
                                    .small()
                                    .label("Cancel")
                                    .accessibility_label("Cancel Assistant Turn")
                                    .tooltip("Cancel Assistant Turn")
                                    .on_click(cx.listener(|this, _, _, cx| this.stop_assistant(cx))),
                            ))
                            .when(!disconnected && !active_turn, |row| row.child(
                                DropdownButton::new("assistant-send-mode")
                                    .primary()
                                    .small()
                                    .disabled(!controls_ready || first_message.is_some())
                                    .button(
                                        Button::new("assistant-send")
                                            .icon(AssetIconName::Send)
                                            .label(if mode_is_run { "Send · Run" } else { "Send · Ask" })
                                            .tooltip(if mode_is_run {
                                                "Send Message · Run automatically"
                                            } else {
                                                "Send Message · Ask before running"
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.send_assistant(window, cx)
                                            })),
                                    )
                                    .dropdown_menu({
                                        let ask = std::rc::Rc::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            if this.displayed_mode() == AssistantExecutionMode::RunAutomatically {
                                                this.toggle_assistant_mode(window, cx);
                                            }
                                        }));
                                        let run = std::rc::Rc::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            if this.displayed_mode() == AssistantExecutionMode::AskBeforeRunning {
                                                this.toggle_assistant_mode(window, cx);
                                            }
                                        }));
                                        move |menu, _, _| menu
                                            .item(PopupMenuItem::new("Ask before running")
                                                .checked(!mode_is_run)
                                                .on_click({ let ask = ask.clone(); move |event, window, cx| ask(event, window, cx) }))
                                            .item(PopupMenuItem::new("Run automatically")
                                                .checked(mode_is_run)
                                                .on_click({ let run = run.clone(); move |event, window, cx| run(event, window, cx) }))
                                    }),
                            )),
                    ),
            )))
            .when(show_threads, |panel| panel.child(self.assistant_thread_list(narrow, width, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn narrow_panes_collapse_threads_until_requested() {
        assert!(!show_thread_list(true, None));
        assert!(show_thread_list(false, None));
        assert!(show_thread_list(true, Some(true)));
        assert!(!show_thread_list(false, Some(false)));
    }

    #[::core::prelude::v1::test]
    fn selecting_a_thread_closes_the_list_only_on_narrow_panes() {
        for narrow in [true, false] {
            let after = thread_list_after_selection(Some(true));
            assert_eq!(show_thread_list(narrow, after), !narrow);
        }
        assert_eq!(thread_list_after_selection(None), None);
    }

    #[::core::prelude::v1::test]
    fn selector_labels_expand_in_priority_order() {
        let widths = [148., 100., 68.];
        assert_eq!(assistant_selector_labels(116., widths), [false; 3]);
        assert_eq!(
            assistant_selector_labels(228., widths),
            [true, false, false]
        );
        assert_eq!(assistant_selector_labels(292., widths), [true, true, false]);
        assert_eq!(assistant_selector_labels(324., widths), [true; 3]);
    }
}
