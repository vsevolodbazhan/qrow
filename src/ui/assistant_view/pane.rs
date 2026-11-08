//! The assistant pane view: header, transcript, approval card, and message
//! field.
//!
//! The pane is its own view, so that typing in the message field or a streamed
//! reply renders the pane again, and not the whole window. It reads the
//! assistant state from Qrow and renders again when Qrow changes. Commands go
//! to Qrow through [`on_qrow`].
use super::*;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};

/// The most rows of the message field in a window `height` high.
pub(super) fn composer_max_rows(height: Pixels, line_height: Pixels) -> usize {
    if line_height <= px(0.) {
        return COMPOSER_MIN_ROWS;
    }
    ((height * COMPOSER_MAX_HEIGHT / line_height).floor() as usize).max(COMPOSER_MIN_ROWS)
}

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

/// The width of a selector button that shows only its icon, in points.
const SELECTOR_ICON_WIDTH: f32 = 24.;
/// The rows the message field shows when it holds a short message.
const COMPOSER_MIN_ROWS: usize = 3;
/// The part of the window height that the message field can grow to. A longer
/// message scrolls inside the field.
const COMPOSER_MAX_HEIGHT: f32 = 0.4;
/// The gap between two selector buttons, in points.
const SELECTOR_GAP: f32 = 4.;
/// What a labeled selector adds to its label: the compact side padding, the
/// icon, the caret, and their gaps, in points. Measured on the rendered
/// composer, with a few points to spare, so a label that shows always fits.
const SELECTOR_CHROME: f32 = 30.;
/// What a labeled button of the composer adds to its label, measured the
/// same way.
const COMPOSER_BUTTON_CHROME: f32 = 24.;

/// Which selectors show their labels when `available` points hold them:
/// the model first, then reasoning, then the tier. `labeled` holds the
/// width of each selector with its label; `None` is a selector that the
/// composer does not show. A selector without its label shows its icon.
pub(super) fn assistant_selector_labels(available: f32, labeled: [Option<f32>; 3]) -> [bool; 3] {
    let shown = labeled.iter().flatten().count() as f32;
    let mut used = shown * SELECTOR_ICON_WIDTH + (shown - 1.).max(0.) * SELECTOR_GAP;
    let mut labels = [false; 3];
    for (index, width) in labeled.iter().enumerate() {
        let Some(width) = width else {
            continue;
        };
        let with_label = used - SELECTOR_ICON_WIDTH + width;
        if with_label > available {
            break;
        }
        used = with_label;
        labels[index] = true;
    }
    labels
}

impl Qrow {
    pub(in crate::ui) fn assistant_transcript_visible(&self, window: &Window, cx: &App) -> bool {
        let pane = self.assistant_pane.read(cx);
        let width = pane
            .layout_width
            .unwrap_or_else(|| self.assistant_width(window.viewport_size().width));
        let narrow = width < self.ui_px(600.);
        self.settings.assistant.enabled
            && self.assistant_state.open
            && !self.activity.read(cx).is_open()
            && (!narrow || !show_thread_list(narrow, pane.thread_list_override))
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
    /// The most rows that the message field grows to at this window height.
    composer_max_rows: usize,
    /// Content width presented by the shell during resize transitions.
    layout_width: Option<Pixels>,
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
                .auto_grow(COMPOSER_MIN_ROWS, COMPOSER_MIN_ROWS)
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
            composer_max_rows: COMPOSER_MIN_ROWS,
            layout_width: None,
            _subscriptions: subscriptions,
        }
    }

    pub(in crate::ui) fn set_layout_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if self.layout_width != Some(width) {
            self.layout_width = Some(width);
            cx.notify();
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

    /// Lets the message field grow to a part of the window height.
    fn sync_composer_rows(&mut self, window: &Window, cx: &mut Context<Self>) {
        let line_height = self
            .composer
            .read(cx)
            .line_height()
            .unwrap_or_else(|| window.line_height());
        let max_rows = composer_max_rows(window.viewport_size().height, line_height);
        if max_rows != self.composer_max_rows {
            self.composer_max_rows = max_rows;
            self.composer.update(cx, |state, cx| {
                state.set_auto_grow(COMPOSER_MIN_ROWS, max_rows, cx)
            });
        }
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
            let width = self
                .layout_width
                .unwrap_or_else(|| qrow.assistant_width(window.viewport_size().width));
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
        self.sync_composer_rows(window, cx);
        let qrow = qrow.read(cx);
        let width = self
            .layout_width
            .unwrap_or_else(|| qrow.assistant_width(window.viewport_size().width));
        self.panel(qrow, width, window, cx).into_any_element()
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
                    move |index, window, cx| {
                        let (Some(qrow_entity), Some(pane)) = (qrow.upgrade(), pane.upgrade())
                        else {
                            return div().into_any_element();
                        };
                        let Some(row) = pane.read(cx).rows.rows.get(index).copied() else {
                            return div().into_any_element();
                        };
                        transcript_row(
                            qrow_entity.read(cx),
                            &qrow,
                            row,
                            index,
                            &pane.downgrade(),
                            window,
                            cx,
                        )
                    },
                )
                // Like the transcript before it, the list has no scrollbar.
                .scrollbar(false)
                .with_jump_button_label("Jump to latest")
                .with_jump_button_renderer(|button| button.accessibility_label("Jump to latest"))
                // The same insets as the message field: 12 pixels at the
                // sides, and 12 pixels above, between, and below the rows.
                .with_list_style(StyleRefinement::default().pt_3().pb_0())
                .with_row_style(StyleRefinement::default().pb_3()),
            )
    }
}

impl AssistantPane {
    /// The transcript of a conversation without messages: what the
    /// assistant does.
    fn empty_transcript(&self, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .id("assistant-empty")
            .test_support()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .items_center()
            .justify_center()
            .px_6()
            .py_8()
            .gap_2()
            .child(
                Icon::new(AssetIconName::Bot)
                    .large()
                    .text_color(cx.theme().muted_foreground),
            )
            // The title and its description read as one group.
            .child(
                v_flex()
                    .items_center()
                    .gap_0p5()
                    .child(div().font_weight(FontWeight::BOLD).child("Assistant"))
                    .child(
                        div()
                            .text_sm()
                            .line_height(rems(1.25))
                            .text_center()
                            .text_color(cx.theme().muted_foreground)
                            .child("The assistant reads the catalog and writes SQL for this tab."),
                    ),
            )
    }

    fn panel(
        &self,
        qrow: &Qrow,
        width: Pixels,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
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
        // A conversation of another harness shows its saved copy. Its next
        // message needs that harness.
        let other_harness = displayed
            .as_deref()
            .filter(|thread| !qrow.conversation_runs(thread))
            .map(|thread| qrow.conversation_harness(thread));
        let harness_name = qrow.settings.assistant.harness.name();
        let has_tiers = qrow.settings.assistant.harness == AssistantHarness::Codex;
        let mode_is_run = qrow.displayed_mode() == AssistantExecutionMode::RunAutomatically;
        let model = qrow.assistant_state.snapshot.as_ref().and_then(|snapshot| {
            qrow.settings
                .assistant
                .choice()
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
                    .choice()
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
                    .choice()
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
                format!("{name}: waiting for {harness_name}")
            }
        };
        // The width of a label at the composer text size, in points of
        // the 100% scale, like the widths of the pane below.
        let label_width = |text: &str| {
            let run = TextRun {
                len: text.len(),
                font: font(cx.theme().font_family.clone()),
                color: cx.theme().foreground,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let width = window
                .text_system()
                .shape_line(
                    SharedString::from(text.to_owned()),
                    window.rem_size() * 0.875,
                    &[run],
                    None,
                )
                .width;
            f32::from(width) / qrow.settings.ui_scale
        };
        let model_width = SELECTOR_CHROME + label_width(&model_label);
        let reasoning_width = SELECTOR_CHROME + label_width(&reasoning_label);
        let tier_width = SELECTOR_CHROME + label_width(&tier_label);
        // Ask first and Send keep their labels; the selectors use the rest.
        let actions_width = 2. * COMPOSER_BUTTON_CHROME
            + label_width(if mode_is_run { "Auto run" } else { "Ask first" })
            + label_width("Send")
            + 8.;
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
                            qrow.settings.assistant.choice().model.as_deref()
                                == Some(candidate.id()),
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
                    qrow.settings.assistant.choice().service_tier.is_none(),
                )];
                options.extend(model.service_tiers().iter().map(|tier| {
                    (
                        tier.name().to_owned(),
                        qrow.settings.assistant.choice().service_tier.as_deref() == Some(tier.id()),
                    )
                }));
                options
            })
            .unwrap_or_default();
        let assistant_entity = self.qrow.clone();
        let conversation_width = width.as_f32() / qrow.settings.ui_scale
            - if show_threads && !narrow { 230. } else { 0. };
        // The composer stays one row: selectors whose labels do not fit
        // beside Send show only their icons, and their tooltips name the value.
        // The composer has 12 points of padding at each side, and the
        // selectors and the actions keep a gap between them.
        let [show_model_label, show_reasoning_label, show_tier_label] = assistant_selector_labels(
            (conversation_width - 24. - actions_width - 12.).max(0.),
            [
                Some(model_width),
                Some(reasoning_width),
                has_tiers.then_some(tier_width),
            ],
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
            .id("assistant-panel")
            .test_support()
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
                            .accessibility_label("New conversation")
                            .tooltip("New conversation")
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
                            .tooltip("More")
                            .disabled(displayed.is_none() || busy)
                            .dropdown_menu({
                                let menu = qrow.conversation_menu(selected, &self.qrow);
                                move |popup, _, _| menu.build(popup)
                            }),
                    )
                    .child(
                        Button::new("assistant-maximize")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .flex_shrink_0()
                            .icon(if qrow.assistant_state.maximized {
                                IconName::Minimize
                            } else {
                                IconName::Maximize
                            })
                            .accessibility_label(if qrow.assistant_state.maximized {
                                "Minimize"
                            } else {
                                "Maximize"
                            })
                            .map(|button| {
                                if qrow.assistant_state.maximized {
                                    button.tooltip_with_action("Minimize", &MinimizeAssistant, None)
                                } else {
                                    button.tooltip_with_action("Maximize", &MaximizeAssistant, None)
                                }
                            })
                            .on_click(on_qrow(&self.qrow, |this, _, window, cx| {
                                this.set_assistant_maximized(!this.assistant_state.maximized, window, cx);
                            })),
                    )
                    .child(
                        Button::new("assistant-toggle-threads")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .flex_shrink_0()
                            .icon(AssetIconName::List)
                            .accessibility_label("Toggle conversation list")
                            .tooltip("Conversations")
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
            .when(!signed_out && self.rows.rows.is_empty() && first_message.is_none(), |panel| {
                panel.child(self.empty_transcript(cx))
            })
            .when(!signed_out && (!self.rows.rows.is_empty() || first_message.is_some()), |panel| {
                panel.child(self.transcript(cx))
            })
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
                                "Run in {tab_title} on {connection}? {}",
                                pending.sql
                            ))
                            .p_3()
                            .gap_2()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(format!("Run in {tab_title} on {connection}?")),
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
                            .when(pending.held_after_web_search, |card| {
                                card.child(
                                    div()
                                        .id("assistant-approval-web-search")
                                        .test_support()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("The assistant searched the web in this turn, so this query waits for you."),
                                )
                            })
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
            .when_some(other_harness, |panel, harness| {
                let name = harness.name();
                panel.child(
                    h_flex()
                        .id("assistant-other-harness")
                        .test_support()
                        .role(Role::Status)
                        .aria_label(format!("This conversation uses {name}."))
                        .px_3()
                        .py_2()
                        .gap_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("This conversation uses {name}.")),
                        )
                        .child(
                            Button::new("assistant-use-harness")
                                .small()
                                .label(format!("Use {name}"))
                                .on_click(on_qrow(&self.qrow, move |this, _, window, cx| {
                                    this.select_assistant_harness(harness, window, cx)
                                })),
                        ),
                )
            })
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
                                .smooth_caret(qrow.settings.assistant_smooth_caret)
                                .smooth_resize(qrow.settings.assistant_smooth_resize && qrow.assistant_state.resizing.is_none())
                                .focus_ring(false)
                                .w_full()
                                .aria_label("Assistant Message")),
                    )
                    .child(
                        h_flex().min_w_0().gap_1()
                            // Until the harness is ready, the selectors have no
                            // values to offer: a status names the wait.
                            .when((!controls_ready || model.is_none()) && !disconnected, |row| row.child(
                                div()
                                    .id("assistant-controls-waiting")
                                    .test_support()
                                    .min_w_0()
                                    .truncate()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Waiting for {harness_name}…")),
                            ))
                            .when(controls_ready && model.is_some(), |row| row.child(
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
                                    .when(has_tiers, |row| row.child(
                                        Button::new("assistant-tier")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::BatteryCharging)
                                            .accessibility_label(control_label("Service Tier", &tier_label))
                                            .tooltip(control_label("Service Tier", &tier_label))
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
                                            }))),
                            ))
                            .child(div().flex_1())
                            .when_some(disconnected_error, |row, error| row.child(
                                Button::new("assistant-reconnect")
                                    .small()
                                    .icon(AssetIconName::RotateCw)
                                    .label("Reconnect")
                                    .accessibility_label(format!("Reconnect to {harness_name}"))
                                    .tooltip(error)
                                    .on_click(on_qrow(&self.qrow, |this, _, window, cx| this.reconnect_assistant(window, cx))),
                            ))
                            .when(!disconnected && active_turn, |row| row.child(
                                Button::new("assistant-stop")
                                    .small()
                                    .label("Cancel")
                                    .accessibility_label("Cancel assistant turn")
                                    .tooltip("Cancel assistant turn")
                                    .on_click(on_qrow(&self.qrow, |this, _, window, cx| this.stop_assistant(window, cx))),
                            ))
                            .when(!disconnected && !active_turn, |row| row.child(
                                h_flex().gap_2().flex_shrink_0()
                                    .child(
                                        Button::new("assistant-send-mode")
                                            .ghost().small()
                                            .label(if mode_is_run { "Auto run" } else { "Ask first" })
                                            .icon(AssetIconName::ChevronDown)
                                            .accessibility_label(if mode_is_run { "SQL Mode: Auto run" } else { "SQL Mode: Ask first" })
                                            .tooltip(if mode_is_run { "Run assistant SQL automatically" } else { "Ask before running assistant SQL" })
                                            .disabled(!controls_ready || first_message.is_some())
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
                                    )
                                    .child(
                                        Button::new("assistant-send")
                                            .primary().small()
                                            .icon(AssetIconName::Send)
                                            .label("Send")
                                            .disabled(!controls_ready || first_message.is_some() || other_harness.is_some())
                                            .map(|mut button| { button.interactivity().tooltip(StatusTooltip::new("Send message", "")
                                                .for_action_in(&SendAssistantMessage, self.composer.focus_handle(cx))); button })
                                            .on_click(on_qrow(&self.qrow, |this, _, window, cx| {
                                                this.send_assistant(window, cx)
                                            })),
                                    )
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
    fn the_message_field_grows_to_a_part_of_the_window() {
        // 40% of 800 points is 320 points: 16 rows of 20 points.
        assert_eq!(composer_max_rows(px(800.), px(20.)), 16);
        assert_eq!(composer_max_rows(px(810.), px(20.)), 16);
        // A short window keeps the rows of a short message.
        assert_eq!(composer_max_rows(px(100.), px(20.)), COMPOSER_MIN_ROWS);
        assert_eq!(composer_max_rows(px(800.), px(0.)), COMPOSER_MIN_ROWS);
    }

    #[::core::prelude::v1::test]
    fn selector_labels_expand_in_priority_order() {
        let widths = [Some(148.), Some(100.), Some(68.)];
        // Three icons take 3 × 24 + 2 × 4 = 80 points.
        assert_eq!(assistant_selector_labels(80., widths), [false; 3]);
        assert_eq!(
            assistant_selector_labels(204., widths),
            [true, false, false]
        );
        assert_eq!(assistant_selector_labels(280., widths), [true, true, false]);
        assert_eq!(assistant_selector_labels(324., widths), [true; 3]);
        // A label that does not fit keeps the next ones as icons too.
        assert_eq!(assistant_selector_labels(203., widths), [false; 3]);
        // Without a tier selector, two labels need less room.
        assert_eq!(
            assistant_selector_labels(256., [Some(148.), Some(100.), None]),
            [true, true, false]
        );
    }
}
