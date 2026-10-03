//! The assistant pane view: header, transcript, approval card, and message
//! field.
//!
//! The pane is its own view, so that typing in the message field or a streamed
//! reply renders the pane again, and not the whole window. It reads the
//! assistant state from Qrow and renders again when Qrow changes. Commands go
//! to Qrow through [`on_qrow`].
use super::*;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};

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
    pub(in crate::ui) fn assistant_transcript_visible(&self, window: &Window, cx: &App) -> bool {
        let narrow = self.assistant_width(window.viewport_size().width) < self.ui_px(600.);
        self.settings.assistant.enabled
            && self.assistant_state.open
            && !self.activity.read(cx).is_open()
            && (!narrow
                || !show_thread_list(narrow, self.assistant_pane.read(cx).thread_list_override))
    }

    /// The dot of a conversation state. An idle conversation has no dot.
    pub(in crate::ui) fn assistant_status_dot(
        &self,
        status: ThreadStatus,
        cx: &App,
    ) -> Option<AnyElement> {
        status.dot_status().map(|status| status.dot(cx))
    }
}

/// A click or other event handler that runs `handler` on Qrow. The pane uses
/// it for Qrow commands. Qrow can then update the pane, because the pane is
/// not in an update of its own when the handler runs.
pub(super) fn on_qrow<E: ?Sized + 'static>(
    qrow: &WeakEntity<Qrow>,
    handler: impl Fn(&mut Qrow, &E, &mut Window, &mut Context<Qrow>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let qrow = qrow.clone();
    move |event, window, cx| {
        let _ = qrow.update(cx, |qrow, cx| handler(qrow, event, window, cx));
    }
}

/// The settings that change the height of transcript rows.
#[derive(Clone, PartialEq)]
struct Typography {
    family: String,
    size: f32,
    line_height: f32,
    scale: f32,
}

impl Typography {
    fn of(qrow: &Qrow) -> Self {
        let settings = &qrow.settings;
        Self {
            family: settings.assistant_font_family.clone(),
            size: settings.assistant_font_size,
            line_height: settings.assistant_line_height,
            scale: settings.ui_scale,
        }
    }
}

/// The assistant pane of a window. It keeps the state that only the pane
/// shows: the message field, the conversation search, and the transcript list.
pub(in crate::ui) struct AssistantPane {
    pub(super) qrow: WeakEntity<Qrow>,
    composer: Entity<TextareaState>,
    pub(super) thread_search: Entity<InputState>,
    /// Lowercase conversation titles and connection names for the search.
    /// The render clears it when the search is empty.
    pub(super) lowercase: RefCell<HashMap<String, String>>,
    pub(super) thread_list_override: Option<bool>,
    transcript: Entity<MessageScrollerState>,
    /// The rows that `transcript` has.
    rows: TranscriptRows,
    /// The settings that change the height of the rows.
    typography: Option<Typography>,
    _subscriptions: [Subscription; 2],
}

impl AssistantPane {
    pub(super) fn read_visible_reply(&self, window: &Window, cx: &mut Context<Self>) {
        let qrow = self.qrow.clone();
        window.defer(cx, move |window, cx| {
            let _ = qrow.update(cx, |qrow, cx| {
                if qrow.assistant_transcript_visible(window, cx)
                    && let Some(thread) = qrow.displayed_thread()
                    && qrow.thread_run_mut(&thread).unread.take().is_some()
                {
                    cx.notify();
                }
            });
        });
    }

    /// Qrow creates the pane while Qrow itself is not ready, so this does not
    /// read Qrow.
    pub fn new(qrow: &Entity<Qrow>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask about your data or describe a query…")
                .submit_on_enter(true)
        });
        let thread_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search conversations…"));
        let transcript = cx.new(|cx| MessageScrollerState::new(0, cx));
        let subscriptions = [
            // The pane reads the assistant state of Qrow.
            cx.observe(qrow, |pane, qrow, cx| {
                let qrow = qrow.read(cx);
                let (rows, typography) = (qrow.assistant_rows(), Typography::of(qrow));
                pane.sync_rows(rows, cx);
                if pane.typography.as_ref() != Some(&typography) {
                    pane.typography = Some(typography);
                    pane.transcript.update(cx, |list, cx| list.remeasure(cx));
                }
                cx.notify();
            }),
            // The jump button follows the scroll position.
            cx.observe(&transcript, |_, _, cx| cx.notify()),
        ];
        Self {
            qrow: qrow.downgrade(),
            composer,
            thread_search,
            lowercase: RefCell::default(),
            thread_list_override: None,
            transcript,
            rows: TranscriptRows::default(),
            typography: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn composer(&self) -> &Entity<TextareaState> {
        &self.composer
    }

    /// Shows the end of the transcript and follows new text there.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |list, cx| list.scroll_to_end(cx));
    }

    /// Closes the thread list of a narrow pane after you select a conversation.
    pub fn thread_selected(&mut self, cx: &mut Context<Self>) {
        self.thread_list_override = thread_list_after_selection(self.thread_list_override);
        cx.notify();
    }

    /// Gives the transcript list the rows of the shown transcript. A new
    /// conversation starts at its end. In the same conversation, the rows
    /// around a change keep their place.
    pub fn sync_rows(&mut self, rows: TranscriptRows, cx: &mut Context<Self>) {
        if rows == self.rows {
            return;
        }
        let old = std::mem::replace(&mut self.rows, rows);
        let new = &self.rows;
        self.transcript.update(cx, |list, cx| {
            if old.source != new.source {
                list.reset(new.rows.len(), cx);
                return;
            }
            let change = row_change(&old.rows, &new.rows);
            if !change.replaced.is_empty() || change.inserted > 0 {
                list.splice(change.replaced, change.inserted, cx);
            }
            for index in change.remeasure {
                list.remeasure_items(index..index + 1, cx);
            }
        });
        cx.notify();
    }
}

impl Render for AssistantPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(qrow) = self.qrow.upgrade() else {
            return div().into_any_element();
        };
        // A layout change can reveal a transcript without a selection action.
        // Defer acknowledgement until this render releases the pane borrow.
        let unread_visible = {
            let qrow = qrow.read(cx);
            let width = qrow.assistant_width(window.viewport_size().width);
            let narrow = width < qrow.ui_px(600.);
            (!narrow || !show_thread_list(narrow, self.thread_list_override))
                && qrow
                    .shown_conversation()
                    .run
                    .is_some_and(|run| run.unread.is_some())
        };
        if unread_visible {
            self.read_visible_reply(window, cx);
        }
        let qrow = qrow.read(cx);
        let width = qrow.assistant_width(window.viewport_size().width);
        self.panel(qrow, width, cx).into_any_element()
    }
}

impl AssistantPane {
    /// The messages of the shown conversation. Only the rows on screen render.
    fn transcript(&self, cx: &Context<Self>) -> impl IntoElement {
        let qrow = self.qrow.clone();
        let pane = cx.weak_entity();
        // GPUI Kit's MessageScroller does not register its root for tests;
        // tests find the transcript through this container.
        div()
            .id("assistant-transcript")
            .test_support()
            .flex_1()
            .min_h_0()
            .child(
                MessageScroller::new(
                    "assistant-messages",
                    self.transcript.clone(),
                    move |index, _, cx| {
                        let (Some(qrow_entity), Some(pane)) = (qrow.upgrade(), pane.upgrade())
                        else {
                            return div().into_any_element();
                        };
                        let Some(row) = pane.read(cx).rows.rows.get(index).copied() else {
                            return div().into_any_element();
                        };
                        transcript_row(qrow_entity.read(cx), &qrow, row, index, cx)
                    },
                )
                // Like the transcript before it, the list has no scrollbar.
                .scrollbar(false)
                .with_jump_button_label("Jump to Latest")
                .with_jump_button_renderer(|button| button.accessibility_label("Jump to Latest"))
                // The same insets as the message field: 12 pixels at the
                // sides, and 12 pixels above, between, and below the rows.
                .with_list_style(StyleRefinement::default().pt_3().pb_0())
                .with_row_style(StyleRefinement::default().pb_3()),
            )
    }
}

impl AssistantPane {
    fn panel(&self, qrow: &Qrow, width: Pixels, cx: &Context<Self>) -> impl IntoElement {
        let narrow = width < qrow.ui_px(600.);
        let show_threads = show_thread_list(narrow, self.thread_list_override);
        let action_size = qrow.ui_px(28.);
        let controls_ready = matches!(qrow.assistant_state.status, Status::Ready);
        let disconnected_error = match &qrow.assistant_state.status {
            Status::Disconnected(error) => Some(error.clone()),
            _ => None,
        };
        let disconnected = disconnected_error.is_some();
        let signed_out = matches!(qrow.assistant_state.status, Status::SignInRequired);
        let shown = qrow.shown_conversation();
        let displayed = shown.thread.clone();
        let selected = displayed.as_deref().unwrap_or("");
        let run = shown.run;
        let active_turn = run.is_some_and(|run| run.active_turn.is_some());
        let first_message = shown.first_message;
        let pending_approval = shown.pending_approval;
        let busy = displayed
            .as_deref()
            .is_some_and(|thread| qrow.thread_status(thread).busy());
        let mode_is_run = qrow.displayed_mode() == AssistantExecutionMode::RunAutomatically;
        let model = qrow.assistant_state.snapshot.as_ref().and_then(|snapshot| {
            qrow.settings
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
                qrow.settings
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
                qrow.settings
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
        let model_options = qrow
            .assistant_state
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .models()
                    .iter()
                    .map(|candidate| {
                        (
                            candidate.display_name().to_owned(),
                            qrow.settings.assistant.model.as_deref() == Some(candidate.id()),
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
                    qrow.settings.assistant.service_tier.is_none(),
                )];
                options.extend(model.service_tiers().iter().map(|tier| {
                    (
                        tier.name().to_owned(),
                        qrow.settings.assistant.service_tier.as_deref() == Some(tier.id()),
                    )
                }));
                options
            })
            .unwrap_or_default();
        let assistant_entity = self.qrow.clone();
        let conversation_width = width.as_f32() / qrow.settings.ui_scale
            - if show_threads && !narrow { 230. } else { 0. };
        let [show_model_label, show_reasoning_label, show_tier_label] = assistant_selector_labels(
            (conversation_width - 196.).max(0.),
            [model_width, reasoning_width, tier_width],
        );
        let displayed_title = displayed
            .as_deref()
            .and_then(|thread| qrow.assistant.conversation(thread))
            .map_or_else(
                || "New conversation".to_owned(),
                |conversation| conversation.title.clone(),
            );
        let title_generating = qrow.assistant_title_generating(selected);
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
                    .h(qrow.ui_px(36.))
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
                            .on_click(on_qrow(&self.qrow, |this, _, window, cx| {
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
                                let menu = qrow.conversation_menu(selected, &self.qrow);
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
                            .on_click(cx.listener(move |pane, _, window, cx| {
                                pane.thread_list_override = Some(!show_threads);
                                pane.read_visible_reply(window, cx);
                                cx.notify();
                            })),
                    )
            )
            .when_some(qrow.assistant_state.notice.as_ref(), |panel, notice| {
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
                    .child(self.sign_in(qrow, cx))))
            .when(!signed_out, |panel| panel.child(self.transcript(cx)))
            .when_some(
                pending_approval,
                |panel, pending| {
                    let tab = qrow.tabs.iter().find(|tab| tab.saved.id == pending.tab_id);
                    let tab_title = tab.map_or("Query tab", |tab| tab.saved.title.as_str());
                    let connection = tab
                        .and_then(|tab| tab.saved.profile)
                        .and_then(|id| qrow.profiles.iter().find(|profile| profile.id == id))
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
                                    .font_family(qrow.settings.editor_font_family.clone())
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
                                            .on_click(on_qrow(&self.qrow, {
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
                                            .on_click(on_qrow(&self.qrow, {
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
                            Textarea::new(&self.composer)
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
                                                            let _ = assistant_entity.update(cx, |this, cx| {
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
                                                            let _ = assistant_entity.update(cx, |this, cx| {
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
                                                            let _ = assistant_entity.update(cx, |this, cx| {
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
                                    .on_click(on_qrow(&self.qrow, |this, _, window, cx| this.reconnect_assistant(window, cx))),
                            ))
                            .when(!disconnected && active_turn, |row| row.child(
                                Button::new("assistant-stop")
                                    .small()
                                    .label("Cancel")
                                    .accessibility_label("Cancel Assistant Turn")
                                    .tooltip("Cancel Assistant Turn")
                                    .on_click(on_qrow(&self.qrow, |this, _, _, cx| this.stop_assistant(cx))),
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
                                            .on_click(on_qrow(&self.qrow, |this, _, window, cx| {
                                                this.send_assistant(window, cx)
                                            })),
                                    )
                                    .dropdown_menu({
                                        let ask = std::rc::Rc::new(on_qrow(&self.qrow, |this, _: &ClickEvent, window, cx| {
                                            if this.displayed_mode() == AssistantExecutionMode::RunAutomatically {
                                                this.toggle_assistant_mode(window, cx);
                                            }
                                        }));
                                        let run = std::rc::Rc::new(on_qrow(&self.qrow, |this, _: &ClickEvent, window, cx| {
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
            .when(show_threads, |panel| panel.child(self.thread_list(qrow, narrow, width, cx)))
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
