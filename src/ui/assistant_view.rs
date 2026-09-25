use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{
    Selectable,
    bubble::{Bubble, BubbleVariant},
    button::DropdownButton,
    h_flex,
    input::{Textarea, TextareaState},
    menu::DropdownMenu,
    message::{Message, MessageAlignment, MessageContent},
    select::{SearchableVec, SelectEvent, SelectState},
    text::{TextView, TextViewStyle},
    v_flex,
};
use qrow::{
    assistant::{
        AccountKind, AssistantEvent, HarnessSnapshot, HistoryTurn, ToolCall, TurnRequest,
        broker::{
            ActionTarget, ConnectionContext, ConnectionState, QueryState, ResultSummary,
            SelectedTabContext, TabSummary, WorkspaceContext, bound_text,
        },
        history_item_text,
        service::{
            Command as AssistantCommand, Event as AssistantServiceEvent, Operation, Service,
        },
        tools,
    },
    model::{AssistantConversation, AssistantTitleSource},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn conversation_age(last_activity: u64, now: u64) -> String {
    if last_activity == 0 {
        return "Earlier".into();
    }
    let elapsed = now.saturating_sub(last_activity);
    if elapsed < 60 {
        "Now".into()
    } else if elapsed < 3_600 {
        format!("{}m", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h", elapsed / 3_600)
    } else if elapsed < 604_800 {
        format!("{}d", elapsed / 86_400)
    } else {
        format!("{}w", elapsed / 604_800)
    }
}

fn show_thread_list(narrow: bool, override_visibility: Option<bool>) -> bool {
    override_visibility.unwrap_or(!narrow)
}

fn reasoning_effort_label(id: &str) -> String {
    match id {
        "xhigh" => "Extra High".into(),
        _ => id
            .split(['_', '-', ' '])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| format!("{}{}", first.to_uppercase(), chars.as_str()))
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn assistant_select_width(label: &str) -> f32 {
    56. + label.chars().count() as f32 * 8.
}

fn assistant_selector_labels(available: f32, widths: [f32; 3]) -> [bool; 3] {
    let icons = 36. * 3. + 8.;
    let model = available >= icons + widths[0] - 36.;
    let reasoning = model && available >= icons + widths[0] + widths[1] - 72.;
    let tier = reasoning && available >= widths.iter().sum::<f32>() + 8.;
    [model, reasoning, tier]
}

#[derive(Clone, Debug)]
pub(super) enum Status {
    Idle,
    Starting,
    SignInRequired,
    Ready,
    Disconnected(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn history_keeps_distinct_messages_from_one_turn() {
        let mut entries = vec![
            TranscriptEntry::new(Speaker::User, "draft".into(), Some("turn-1".into())),
            TranscriptEntry::new(Speaker::Assistant, "streamed".into(), Some("turn-1".into())),
        ];
        merge_history(
            &mut entries,
            vec![HistoryTurn {
                id: "turn-1".into(),
                status: "completed".into(),
                items: vec![
                    json!({"type":"userMessage","content":[{"type":"text","text":"question"}]}),
                    json!({"type":"agentMessage","text":"first"}),
                    json!({"type":"agentMessage","text":"second"}),
                ],
            }],
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].text, "question");
        assert_eq!(entries[1].text, "first");
        assert_eq!(entries[2].text, "second");
    }

    #[::core::prelude::v1::test]
    fn conversation_age_handles_unknown_and_recent_activity() {
        assert_eq!(conversation_age(0, 1_000), "Earlier");
        assert_eq!(conversation_age(990, 1_000), "Now");
        assert_eq!(conversation_age(880, 1_000), "2m");
        assert_eq!(conversation_age(1_000, 999), "Now");
    }

    #[::core::prelude::v1::test]
    fn narrow_panes_collapse_threads_until_requested() {
        assert!(!show_thread_list(true, None));
        assert!(show_thread_list(false, None));
        assert!(show_thread_list(true, Some(true)));
        assert!(!show_thread_list(false, Some(false)));
    }

    #[::core::prelude::v1::test]
    fn reasoning_labels_keep_codex_ids_separate_from_display_text() {
        assert_eq!(reasoning_effort_label("low"), "Low");
        assert_eq!(reasoning_effort_label("medium"), "Medium");
        assert_eq!(reasoning_effort_label("high"), "High");
        assert_eq!(reasoning_effort_label("xhigh"), "Extra High");
        assert_eq!(reasoning_effort_label("max"), "Max");
    }

    #[::core::prelude::v1::test]
    fn selector_labels_expand_in_priority_order() {
        let widths = [148., 100., 68.];
        assert_eq!(assistant_selector_labels(116., widths), [false; 3]);
        assert_eq!(assistant_selector_labels(228., widths), [true, false, false]);
        assert_eq!(assistant_selector_labels(292., widths), [true, true, false]);
        assert_eq!(assistant_selector_labels(324., widths), [true; 3]);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Speaker {
    User,
    Assistant,
    Activity,
    Error,
}

#[derive(Clone, Debug)]
pub(super) struct TranscriptEntry {
    id: Uuid,
    pub speaker: Speaker,
    pub text: String,
    pub turn_id: Option<String>,
    pub detail: Option<String>,
    pub expanded: bool,
}

impl TranscriptEntry {
    pub fn new(speaker: Speaker, text: String, turn_id: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            speaker,
            text,
            turn_id,
            detail: None,
            expanded: false,
        }
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }
}

fn merge_history(entries: &mut Vec<TranscriptEntry>, turns: Vec<HistoryTurn>) {
    let mut matched = BTreeSet::new();
    for turn in turns {
        for item in turn.items {
            let Some((speaker, text)) = history_item_text(&item) else {
                continue;
            };
            let speaker = if speaker == "user" {
                Speaker::User
            } else {
                Speaker::Assistant
            };
            if let Some(index) = entries
                .iter()
                .enumerate()
                .find(|(index, entry)| {
                    !matched.contains(index)
                        && entry.speaker == speaker
                        && entry.turn_id.as_deref() == Some(&turn.id)
                })
                .map(|(index, _)| index)
            {
                entries[index].text = text;
                matched.insert(index);
            } else {
                entries.push(TranscriptEntry::new(speaker, text, Some(turn.id.clone())));
                matched.insert(entries.len() - 1);
            }
        }
    }
}

pub(super) struct PendingQuery {
    pub call: ToolCall,
    pub tab_id: Uuid,
    pub revision: u64,
    pub sql: String,
    pub approved: bool,
    pub started: bool,
    pub kind: PendingQueryKind,
    pub activity_index: Option<usize>,
    pub detached: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingQueryKind {
    Run,
    Fetch,
}

pub(super) struct AssistantPanelState {
    pub open: bool,
    pub status: Status,
    pub service: Option<Service>,
    pub snapshot: Option<HarnessSnapshot>,
    pub composer: Entity<TextareaState>,
    _composer_subscription: Subscription,
    thread_search: Entity<InputState>,
    _thread_search_subscription: Subscription,
    thread_list_override: Option<bool>,
    mode_select: Entity<SelectState<SearchableVec<String>>>,
    model_select: Entity<SelectState<SearchableVec<String>>>,
    reasoning_select: Entity<SelectState<SearchableVec<String>>>,
    tier_select: Entity<SelectState<SearchableVec<String>>>,
    _select_subscriptions: Vec<Subscription>,
    pub transcripts: BTreeMap<String, Vec<TranscriptEntry>>,
    pub older_cursors: BTreeMap<String, String>,
    pub loaded_cursors: BTreeMap<String, BTreeSet<String>>,
    pub loading_older: bool,
    pub active_turn: Option<String>,
    pub target: Option<ActionTarget>,
    pub pending_query: Option<PendingQuery>,
    pub scroll: ScrollHandle,
    pub resizing: Option<(Point<Pixels>, Pixels)>,
    pub auto_hidden_sidebar: bool,
    pub unread: bool,
    pub notice: Option<String>,
    pub previous_focus: Option<FocusHandle>,
    pub rename_input: Option<Entity<InputState>>,
    pub pending_rename: Option<(String, String)>,
    pub creating_conversation: bool,
}

impl AssistantPanelState {
    pub fn new(window: &mut Window, cx: &mut Context<Qrow>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask about SQL or this workspace…")
                .submit_on_enter(true)
        });
        let subscription = cx.subscribe_in(
            &composer,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { shift: false, .. } = event {
                    this.send_assistant(window, cx);
                } else if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            },
        );
        let thread_search =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search conversations…"));
        let thread_search_subscription =
            cx.subscribe_in(&thread_search, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            });
        let mut make_select = |cx: &mut Context<Qrow>| {
            cx.new(|cx| SelectState::new(SearchableVec::new(vec![]), None, window, cx))
        };
        let model_select = make_select(cx);
        let reasoning_select = make_select(cx);
        let tier_select = make_select(cx);
        let mode_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec!["Ask".to_owned(), "Run".to_owned()]),
                Some(gpui_kit::component::IndexPath::default().row(0)),
                window,
                cx,
            )
        });
        let select_subscriptions = vec![
            cx.subscribe_in(
                &mode_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event {
                        let automatic = this
                            .assistant
                            .selected_thread
                            .as_ref()
                            .and_then(|id| {
                                this.assistant
                                    .conversations
                                    .iter()
                                    .find(|conversation| &conversation.thread_id == id)
                            })
                            .is_some_and(|conversation| {
                                conversation.execution_mode
                                    == qrow::model::AssistantExecutionMode::RunAutomatically
                            });
                        if (label == "Run") != automatic {
                            this.toggle_assistant_mode(window, cx);
                            this.sync_assistant_selectors(window, cx);
                        }
                    }
                },
            ),
            cx.subscribe_in(
                &model_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event {
                        this.select_assistant_model(label, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &reasoning_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event {
                        this.select_assistant_reasoning(label, window, cx);
                    }
                },
            ),
            cx.subscribe_in(
                &tier_select,
                window,
                |this, _, event: &SelectEvent<SearchableVec<String>>, window, cx| {
                    if let SelectEvent::Confirm(Some(label)) = event {
                        this.select_assistant_tier(label, window, cx);
                    }
                },
            ),
        ];
        Self {
            open: false,
            status: Status::Idle,
            service: None,
            snapshot: None,
            composer,
            _composer_subscription: subscription,
            thread_search,
            _thread_search_subscription: thread_search_subscription,
            thread_list_override: None,
            mode_select,
            model_select,
            reasoning_select,
            tier_select,
            _select_subscriptions: select_subscriptions,
            transcripts: BTreeMap::new(),
            older_cursors: BTreeMap::new(),
            loaded_cursors: BTreeMap::new(),
            loading_older: false,
            active_turn: None,
            target: None,
            pending_query: None,
            scroll: ScrollHandle::new(),
            resizing: None,
            auto_hidden_sidebar: false,
            unread: false,
            notice: None,
            previous_focus: None,
            rename_input: None,
            pending_rename: None,
            creating_conversation: false,
        }
    }

    pub fn shutdown(&mut self) {
        if let Some(mut service) = self.service.take() {
            let _ = service.shutdown_and_wait(Duration::from_secs(2));
        }
    }

    pub fn shutdown_demo(&mut self, ids: Vec<String>) {
        if let Some(mut service) = self.service.take()
            && let Err(error) = service.shutdown_and_delete(ids, Duration::from_secs(2))
        {
            eprintln!("Could not clean up demo assistant conversations: {error}");
        }
    }
}

fn discover_codex(configured: Option<&str>) -> Result<PathBuf, String> {
    if let Some(path) = configured {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path).ok_or_else(|| {
            "The configured Codex executable was not found. Choose another path in Settings.".into()
        });
    }
    let mut candidates = Vec::new();
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("codex")));
    }
    candidates.extend(
        [
            "/opt/homebrew/bin/codex",
            "/usr/local/bin/codex",
            "/usr/bin/codex",
        ]
        .map(PathBuf::from),
    );
    candidates
        .into_iter()
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "Codex was not found. Install Codex, or select its executable in Settings.".into()
        })
}

impl Qrow {
    fn load_older_assistant_messages(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.assistant.selected_thread.clone() else {
            return;
        };
        let Some(cursor) = self.assistant_panel.older_cursors.get(&thread_id).cloned() else {
            return;
        };
        if self
            .assistant_panel
            .loaded_cursors
            .entry(thread_id.clone())
            .or_default()
            .contains(&cursor)
        {
            self.assistant_panel.older_cursors.remove(&thread_id);
            self.assistant_panel.notice =
                Some("Codex repeated a conversation page. Older messages cannot be loaded.".into());
            cx.notify();
            return;
        }
        if self.assistant_panel.loading_older
            || self.assistant_panel.active_turn.is_some()
            || self.assistant_panel.pending_query.is_some()
        {
            return;
        }
        if self.assistant_command(
            AssistantCommand::ReadOlder {
                thread_id: thread_id.clone(),
                cursor: cursor.clone(),
            },
            cx,
        ) {
            self.assistant_panel
                .loaded_cursors
                .entry(thread_id)
                .or_default()
                .insert(cursor);
            self.assistant_panel.loading_older = true;
            cx.notify();
        }
    }
    fn create_assistant_conversation(&mut self, cx: &mut Context<Self>) {
        if !self.assistant_panel.creating_conversation
            && self.assistant_command(AssistantCommand::Create(tools::definitions()), cx)
        {
            self.assistant_panel.creating_conversation = true;
        }
    }
    fn assistant_transcript_near_bottom(&self) -> bool {
        let scroll = &self.assistant_panel.scroll;
        scroll.offset().y + scroll.max_offset().y <= self.ui_px(32.)
    }
    fn begin_assistant_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(conversation) = self.assistant.conversations.iter().find(|conversation| {
            Some(&conversation.thread_id) == self.assistant.selected_thread.as_ref()
        }) else {
            return;
        };
        let title = conversation.title.clone();
        self.assistant_panel.rename_input =
            Some(cx.new(|cx| InputState::new(window, cx).default_value(title)));
        cx.notify();
    }

    fn save_assistant_rename(&mut self, cx: &mut Context<Self>) {
        let (Some(id), Some(input)) = (
            self.assistant.selected_thread.clone(),
            self.assistant_panel.rename_input.as_ref(),
        ) else {
            return;
        };
        let title = input.read(cx).value().trim().to_owned();
        if title.is_empty() || title.chars().count() > 120 {
            self.assistant_panel.notice = Some("Use a title with 1 to 120 characters.".into());
            cx.notify();
            return;
        }
        if self.assistant_command(
            AssistantCommand::Rename {
                thread_id: id.clone(),
                title: title.clone(),
            },
            cx,
        ) {
            self.assistant_panel.pending_rename = Some((id, title));
            self.assistant_panel.rename_input = None;
            cx.notify();
        }
    }

    fn confirm_assistant_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread_id) = self.assistant.selected_thread.clone() else {
            return;
        };
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            let id = thread_id.clone();
            alert.title("Delete assistant conversation?")
                .description("Codex will delete this conversation. Query tabs, SQL, sessions, Logs, and results will not change.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-delete-assistant-conversation").label("Cancel").on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-delete-assistant-conversation").with_variant(ButtonVariant::Danger).label("Delete")
                        .on_click(move |_, window, cx| {
                            let _ = confirm.update(cx, |this, cx| { this.assistant_command(AssistantCommand::Delete(id.clone()), cx); });
                            window.close_dialog(cx);
                        })))
        });
    }

    fn conversation_label(&self, conversation: &AssistantConversation) -> String {
        if self
            .assistant
            .conversations
            .iter()
            .filter(|other| other.title == conversation.title)
            .count()
            == 1
        {
            conversation.title.clone()
        } else {
            format!(
                "{} · {}",
                conversation.title,
                conversation.thread_id.chars().take(8).collect::<String>()
            )
        }
    }

    fn sync_assistant_selectors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mode_index = self
            .assistant
            .selected_thread
            .as_ref()
            .and_then(|id| {
                self.assistant
                    .conversations
                    .iter()
                    .find(|conversation| &conversation.thread_id == id)
            })
            .is_some_and(|conversation| {
                conversation.execution_mode == qrow::model::AssistantExecutionMode::RunAutomatically
            });
        self.assistant_panel.mode_select.update(cx, |state, cx| {
            state.set_selected_value(
                &if mode_index {
                    "Run".to_owned()
                } else {
                    "Ask".to_owned()
                },
                window,
                cx,
            );
        });
        let Some(snapshot) = self.assistant_panel.snapshot.clone() else {
            return;
        };
        let models = snapshot.models();
        if self
            .settings
            .assistant
            .model
            .as_ref()
            .is_some_and(|id| !models.iter().any(|model| model.id() == id))
        {
            self.settings.assistant.model = None;
            self.settings.assistant.reasoning_effort = None;
            self.settings.assistant.service_tier = None;
            self.assistant_panel.notice =
                Some("Saved model is unavailable. Codex default model selected.".into());
            self.changed(cx);
        }
        let default_model = models
            .iter()
            .find(|model| model.is_default())
            .or_else(|| models.first());
        if self.settings.assistant.model.is_none()
            && let Some(model) = default_model
        {
            self.settings.assistant.model = Some(model.id().to_owned());
            self.changed(cx);
        }
        let model = self
            .settings
            .assistant
            .model
            .as_deref()
            .and_then(|id| models.iter().find(|model| model.id() == id))
            .or(default_model);
        let model_labels: Vec<_> = models
            .iter()
            .map(|model| model.display_name().to_owned())
            .collect();
        let selected_model_label = model.map(|model| model.display_name().to_owned());
        self.assistant_panel.model_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(model_labels), window, cx);
            if let Some(label) = selected_model_label {
                state.set_selected_value(&label, window, cx);
            }
        });
        let efforts = model.map(|model| model.reasoning_efforts()).unwrap_or(&[]);
        if self
            .settings
            .assistant
            .reasoning_effort
            .as_ref()
            .is_some_and(|id| !efforts.iter().any(|effort| effort.id() == id))
        {
            self.settings.assistant.reasoning_effort = None;
            self.assistant_panel.notice =
                Some("Saved reasoning level is unavailable. Codex default is in use.".into());
            self.changed(cx);
        }
        let effort_labels: Vec<_> = efforts
            .iter()
            .map(|effort| reasoning_effort_label(effort.id()))
            .collect();
        let selected_effort = self
            .settings
            .assistant
            .reasoning_effort
            .as_deref()
            .or_else(|| model.map(|model| model.default_reasoning_effort()))
            .and_then(|id| efforts.iter().find(|effort| effort.id() == id))
            .or_else(|| efforts.first())
            .map(|effort| reasoning_effort_label(effort.id()));
        self.assistant_panel
            .reasoning_select
            .update(cx, |state, cx| {
                state.set_items(SearchableVec::new(effort_labels), window, cx);
                if let Some(selected) = selected_effort {
                    state.set_selected_value(&selected, window, cx);
                }
            });
        let tiers = model.map(|model| model.service_tiers()).unwrap_or(&[]);
        if self
            .settings
            .assistant
            .service_tier
            .as_ref()
            .is_some_and(|id| !tiers.iter().any(|tier| tier.id() == id))
        {
            self.settings.assistant.service_tier = None;
            self.assistant_panel.notice =
                Some("Saved service tier is unavailable. Codex default is in use.".into());
            self.changed(cx);
        }
        let mut tier_labels = vec!["Default".to_owned()];
        tier_labels.extend(tiers.iter().map(|tier| tier.name().to_owned()));
        let selected_tier = self
            .settings
            .assistant
            .service_tier
            .as_ref()
            .and_then(|id| tiers.iter().find(|tier| tier.id() == id))
            .map(|tier| tier.name().to_owned())
            .unwrap_or_else(|| "Default".to_owned());
        self.assistant_panel.tier_select.update(cx, |state, cx| {
            state.set_items(SearchableVec::new(tier_labels), window, cx);
            state.set_selected_value(&selected_tier, window, cx);
        });
    }

    fn select_assistant_thread(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_panel.active_turn.is_some() {
            return;
        }
        self.assistant.selected_thread = Some(id.to_owned());
        self.assistant_command(AssistantCommand::Resume(id.to_owned()), cx);
        self.sync_assistant_selectors(window, cx);
        if self.assistant_panel.thread_list_override == Some(true) {
            self.assistant_panel.thread_list_override = Some(false);
        }
        self.changed(cx);
    }

    fn select_assistant_model(&mut self, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(model_id) = self
            .assistant_panel
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .models()
                    .iter()
                    .find(|model| model.display_name() == label)
            })
            .map(|model| model.id().to_owned())
        else {
            return;
        };
        self.settings.assistant.model = Some(model_id);
        self.settings.assistant.reasoning_effort = None;
        self.settings.assistant.service_tier = None;
        self.sync_assistant_selectors(window, cx);
        self.changed(cx);
    }

    fn select_assistant_reasoning(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings.assistant.reasoning_effort = self
            .assistant_panel
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                self.settings
                    .assistant
                    .model
                    .as_deref()
                    .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                    .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
                    .or_else(|| snapshot.models().first())
            })
            .and_then(|model| {
                model
                    .reasoning_efforts()
                    .iter()
                    .find(|effort| reasoning_effort_label(effort.id()) == label)
            })
            .map(|effort| effort.id().to_owned());
        self.sync_assistant_selectors(window, cx);
        self.changed(cx);
    }

    fn select_assistant_tier(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tier = if label == "Default" {
            None
        } else {
            let Some(tier) = self.assistant_panel.snapshot.as_ref().and_then(|snapshot| {
                let model = self
                    .settings
                    .assistant
                    .model
                    .as_deref()
                    .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                    .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
                    .or_else(|| snapshot.models().first())?;
                model
                    .service_tiers()
                    .iter()
                    .find(|tier| tier.name() == label)
                    .map(|tier| tier.id().to_owned())
            }) else {
                return;
            };
            Some(tier)
        };
        self.settings.assistant.service_tier = tier;
        self.sync_assistant_selectors(window, cx);
        self.changed(cx);
    }
    pub(super) fn toggle_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.assistant.enabled {
            return;
        }
        self.assistant_panel.open = !self.assistant_panel.open;
        if self.assistant_panel.open {
            self.assistant_panel.previous_focus = window.focused(cx);
            let needed =
                self.sidebar_width + self.ui_px(self.settings.assistant.panel_width + 420.);
            if self.sidebar && window.viewport_size().width < needed {
                self.sidebar = false;
                self.assistant_panel.auto_hidden_sidebar = true;
            }
            self.assistant_panel.unread = false;
            self.start_assistant(cx);
            self.assistant_panel
                .composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        } else if self.assistant_panel.auto_hidden_sidebar {
            self.sidebar = true;
            self.assistant_panel.auto_hidden_sidebar = false;
        }
        if !self.assistant_panel.open
            && let Some(focus) = self.assistant_panel.previous_focus.take()
        {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn start_assistant(&mut self, cx: &mut Context<Self>) {
        if self.assistant_panel.service.is_some() {
            return;
        }
        let executable = match discover_codex(self.settings.assistant.codex_executable.as_deref()) {
            Ok(path) => path,
            Err(error) => {
                self.assistant_panel.status = Status::Disconnected(error);
                cx.notify();
                return;
            }
        };
        let wake = self.wake.clone();
        match Service::launch(
            executable,
            Arc::new(move || {
                let _ = wake.try_send(());
            }),
        ) {
            Ok(service) => {
                self.assistant_panel.service = Some(service);
                self.assistant_panel.status = Status::Starting;
            }
            Err(error) => {
                self.assistant_panel.status =
                    Status::Disconnected(format!("Could not start assistant: {error}"))
            }
        }
        cx.notify();
    }

    pub(super) fn reconnect_assistant(&mut self, cx: &mut Context<Self>) {
        self.assistant_panel.shutdown();
        self.assistant_panel.active_turn = None;
        self.assistant_panel.target = None;
        self.assistant_panel.pending_query = None;
        self.start_assistant(cx);
    }

    pub(super) fn stop_assistant(&mut self, cx: &mut Context<Self>) {
        let (Some(thread_id), Some(turn_id)) = (
            self.assistant.selected_thread.clone(),
            self.assistant_panel.active_turn.clone(),
        ) else {
            return;
        };
        self.assistant_command(AssistantCommand::Interrupt { thread_id, turn_id }, cx);
    }

    pub(super) fn toggle_assistant_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(thread_id) = self.assistant.selected_thread.clone() else {
            return;
        };
        let Some(conversation) = self
            .assistant
            .conversations
            .iter()
            .find(|conversation| conversation.thread_id == thread_id)
        else {
            return;
        };
        if conversation.execution_mode == qrow::model::AssistantExecutionMode::RunAutomatically {
            if let Some(conversation) = self
                .assistant
                .conversations
                .iter_mut()
                .find(|conversation| conversation.thread_id == thread_id)
            {
                conversation.execution_mode = qrow::model::AssistantExecutionMode::AskBeforeRunning;
                self.changed(cx);
            }
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            let thread_id = thread_id.clone();
            alert.title("Run assistant queries automatically?")
                .description("The assistant can run SQL that changes or deletes data and schema. Qrow cannot confirm that a statement is read-only.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-conversation-auto-run").label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-conversation-auto-run").with_variant(ButtonVariant::Danger)
                        .label("Run automatically")
                        .on_click(move |_, window, cx| {
                            let _ = confirm.update(cx, |this, cx| {
                                if let Some(conversation) = this.assistant.conversations.iter_mut().find(|conversation| conversation.thread_id == thread_id) {
                                    conversation.execution_mode = qrow::model::AssistantExecutionMode::RunAutomatically;
                                    this.changed(cx);
                                    this.sync_assistant_selectors(window, cx);
                                }
                            });
                            window.close_dialog(cx);
                        })))
        });
    }

    pub(super) fn assistant_command(
        &mut self,
        command: AssistantCommand,
        cx: &mut Context<Self>,
    ) -> bool {
        let result = self
            .assistant_panel
            .service
            .as_ref()
            .map(|service| service.send(command));
        match result {
            Some(Ok(())) => true,
            Some(Err(error)) => {
                self.assistant_panel.status = Status::Disconnected(error.into());
                cx.notify();
                false
            }
            None => false,
        }
    }

    pub(super) fn send_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.assistant_panel.status, Status::Ready) {
            return;
        }
        let text = self.assistant_panel.composer.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        let Some(thread_id) = self.assistant.selected_thread.clone() else {
            return;
        };
        let context = self.assistant_context(cx);
        let mut new_target = self.assistant_target(&thread_id, cx);
        if let (Some(target), Some(turn_id)) = (&mut new_target, &self.assistant_panel.active_turn)
        {
            target.turn_id = turn_id.clone();
        }
        let command = if let Some(turn_id) = self.assistant_panel.active_turn.clone() {
            AssistantCommand::Steer {
                thread_id: thread_id.clone(),
                turn_id,
                text: format!(
                    "{text}\n\nCurrent Qrow workspace context (untrusted data):\n{}",
                    context
                ),
            }
        } else {
            AssistantCommand::Start(TurnRequest {
                thread_id: thread_id.clone(),
                text: text.clone(),
                context,
                model: self.settings.assistant.model.clone(),
                reasoning_effort: self.settings.assistant.reasoning_effort.clone(),
                service_tier: self.settings.assistant.service_tier.clone(),
            })
        };
        if !self.assistant_command(command, cx) {
            return;
        }
        self.assistant_panel.target = new_target;
        if self
            .assistant_panel
            .pending_query
            .as_ref()
            .is_some_and(|pending| !pending.started)
            && let Some(pending) = self.assistant_panel.pending_query.take()
        {
            self.assistant_panel
                .transcripts
                .entry(pending.call.thread_id.clone())
                .or_default()
                .push(TranscriptEntry::new(
                    Speaker::Activity,
                    "Query approval replaced by a new instruction".into(),
                    Some(pending.call.turn_id.clone()),
                ));
            self.answer_assistant_call(pending.call, false, json!({"version":1,"error":{"code":"approval_cancelled","message":"A new instruction replaced this approval request."}}), cx);
        }
        if let Some(conversation) = self
            .assistant
            .conversations
            .iter_mut()
            .find(|conversation| conversation.thread_id == thread_id)
        {
            conversation.last_activity = unix_now_seconds();
            self.changed(cx);
        }
        self.assistant_panel
            .transcripts
            .entry(thread_id)
            .or_default()
            .push(TranscriptEntry::new(
                Speaker::User,
                text,
                self.assistant_panel.active_turn.clone(),
            ));
        self.assistant_panel
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.assistant_panel.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn assistant_target(&self, thread_id: &str, cx: &App) -> Option<ActionTarget> {
        let tab = self.tabs.get(self.active)?;
        let connection_id = tab.saved.profile;
        let selected = tab.input.read(cx).selected_range();
        Some(ActionTarget {
            conversation_id: thread_id.into(),
            turn_id: String::new(),
            tab_id: tab.saved.id,
            connection_id,
            selected_range: (!selected.is_empty()).then_some(selected),
        })
    }

    pub(super) fn assistant_context(&self, cx: &App) -> Value {
        let connections = self
            .profiles
            .iter()
            .map(|profile| {
                let state = if self
                    .tabs
                    .iter()
                    .any(|tab| tab.saved.profile == Some(profile.id) && tab.busy)
                {
                    ConnectionState::Busy
                } else if self
                    .tabs
                    .iter()
                    .any(|tab| tab.saved.profile == Some(profile.id) && tab.connected)
                {
                    ConnectionState::Connected
                } else {
                    ConnectionState::Disconnected
                };
                ConnectionContext {
                    id: profile.id,
                    name: if self.demo {
                        "Demo data".into()
                    } else {
                        profile.name.clone()
                    },
                    connector: "spark_kyuubi",
                    initial_database: profile.database.clone(),
                    state,
                }
            })
            .collect();
        let tabs = self
            .tabs
            .iter()
            .map(|tab| TabSummary {
                id: tab.saved.id,
                title: tab.saved.title.clone(),
                connection_id: tab.saved.profile,
                state: if tab.cancelling {
                    QueryState::Cancelling
                } else if tab.busy {
                    QueryState::Running
                } else if tab.status.starts_with("Error") || tab.status.starts_with("Rejected") {
                    QueryState::Failed
                } else if tab.current_execution.is_some() {
                    QueryState::Finished
                } else {
                    QueryState::Idle
                },
            })
            .collect::<Vec<_>>();
        let selected_tab = self.tabs.get(self.active).map(|tab| {
            let results = tab.table.read(cx);
            let results = results.delegate();
            let selection = tab.input.read(cx).selected_range();
            let error = tab
                .output
                .latest_error()
                .map(|entry| bound_text(&entry.text).0);
            SelectedTabContext {
                tab: tabs[self.active].clone(),
                sql: tab.input.read(cx).value().to_string(),
                selected_range: (!selection.is_empty()).then_some(selection),
                editor_revision: tab.revision,
                results: ResultSummary {
                    columns: results
                        .columns
                        .iter()
                        .map(|column| column.name.clone())
                        .collect(),
                    downloaded_rows: results.rows.len(),
                    more_rows_available: tab.more,
                },
                latest_error: error,
            }
        });
        serde_json::to_value(WorkspaceContext::new(connections, tabs, selected_tab))
            .unwrap_or(json!({"version": 1}))
    }

    pub(super) fn tick_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let events: Vec<_> = self
            .assistant_panel
            .service
            .as_ref()
            .map(|service| service.events.try_iter().collect())
            .unwrap_or_default();
        let changed = !events.is_empty();
        for event in events {
            self.handle_assistant_event(event, window, cx);
        }
        self.update_assistant_query_progress(cx);
        self.complete_assistant_query(cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn update_assistant_snapshot(
        &mut self,
        snapshot: HarnessSnapshot,
        initial: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant_panel.status = if matches!(
            snapshot.account().kind(),
            AccountKind::ChatGpt { .. } | AccountKind::ApiKey
        ) {
            Status::Ready
        } else {
            Status::SignInRequired
        };
        self.assistant_panel.snapshot = Some(snapshot);
        self.sync_assistant_selectors(window, cx);
        if initial && matches!(self.assistant_panel.status, Status::Ready) {
            if let Some(thread) = self.assistant.selected_thread.clone() {
                self.assistant_command(AssistantCommand::Resume(thread), cx);
            } else {
                self.create_assistant_conversation(cx);
            }
        } else if !initial
            && matches!(self.assistant_panel.status, Status::Ready)
            && self.assistant.selected_thread.is_none()
        {
            self.create_assistant_conversation(cx);
        }
    }

    fn handle_assistant_event(
        &mut self,
        event: AssistantServiceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            AssistantServiceEvent::Ready(snapshot) => {
                self.update_assistant_snapshot(snapshot, true, window, cx)
            }
            AssistantServiceEvent::Snapshot(snapshot) => {
                self.update_assistant_snapshot(snapshot, false, window, cx)
            }
            AssistantServiceEvent::Created(conversation) => {
                self.assistant_panel.creating_conversation = false;
                let mut entry = AssistantConversation::new(
                    conversation.id.clone(),
                    self.settings.assistant.default_execution_mode,
                );
                entry.last_activity = unix_now_seconds();
                self.assistant.conversations.push(entry);
                self.assistant.selected_thread = Some(conversation.id);
                self.sync_assistant_selectors(window, cx);
                self.changed(cx);
            }
            AssistantServiceEvent::Resumed(conversation) => {
                self.assistant_command(AssistantCommand::Read(conversation.id), cx);
            }
            AssistantServiceEvent::History(history) => {
                let thread = history.conversation.id;
                if let Some(cursor) = history.older_cursor {
                    if !self.assistant_panel.loaded_cursors.contains_key(&thread) {
                        self.assistant_panel
                            .older_cursors
                            .insert(thread.clone(), cursor);
                    }
                } else {
                    self.assistant_panel.older_cursors.remove(&thread);
                }
                let entries = self.assistant_panel.transcripts.entry(thread).or_default();
                merge_history(entries, history.turns);
            }
            AssistantServiceEvent::HistoryPage(page) => {
                self.assistant_panel.loading_older = false;
                if let Some(cursor) = page.older_cursor {
                    self.assistant_panel
                        .older_cursors
                        .insert(page.thread_id.clone(), cursor);
                } else {
                    self.assistant_panel.older_cursors.remove(&page.thread_id);
                }
                let older: Vec<_> = page
                    .turns
                    .into_iter()
                    .flat_map(|turn| {
                        turn.items.into_iter().filter_map(move |item| {
                            let (speaker, text) = history_item_text(&item)?;
                            Some(TranscriptEntry::new(
                                if speaker == "user" {
                                    Speaker::User
                                } else {
                                    Speaker::Assistant
                                },
                                text,
                                Some(turn.id.clone()),
                            ))
                        })
                    })
                    .collect();
                self.assistant_panel
                    .transcripts
                    .entry(page.thread_id)
                    .or_default()
                    .splice(0..0, older);
            }
            AssistantServiceEvent::TurnStarted { thread_id, turn } => {
                if let Some(entry) =
                    self.assistant_panel
                        .transcripts
                        .get_mut(&thread_id)
                        .and_then(|entries| {
                            entries.iter_mut().rev().find(|entry| {
                                entry.speaker == Speaker::User && entry.turn_id.is_none()
                            })
                        })
                {
                    entry.turn_id = Some(turn.id.clone());
                }
                if self.assistant.selected_thread.as_deref() == Some(&thread_id) {
                    self.assistant_panel.active_turn = Some(turn.id.clone());
                    if let Some(target) = &mut self.assistant_panel.target {
                        target.turn_id = turn.id;
                    }
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::MessageDelta {
                thread_id,
                turn_id,
                text,
            }) => {
                let entries = self
                    .assistant_panel
                    .transcripts
                    .entry(thread_id)
                    .or_default();
                if let Some(last) = entries.last_mut().filter(|entry| {
                    entry.speaker == Speaker::Assistant
                        && entry.turn_id.as_deref() == Some(&turn_id)
                }) {
                    last.text.push_str(&text);
                } else {
                    entries.push(TranscriptEntry::new(
                        Speaker::Assistant,
                        text,
                        Some(turn_id),
                    ));
                }
                if self.assistant_transcript_near_bottom() {
                    self.assistant_panel.scroll.scroll_to_bottom();
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::TurnCompleted {
                thread_id,
                turn,
                error,
            }) => {
                if self
                    .assistant_panel
                    .pending_query
                    .as_ref()
                    .is_some_and(|pending| {
                        pending.call.thread_id == thread_id && pending.call.turn_id == turn.id
                    })
                {
                    if self
                        .assistant_panel
                        .pending_query
                        .as_ref()
                        .is_some_and(|pending| pending.started)
                    {
                        if let Some(pending) = &mut self.assistant_panel.pending_query {
                            pending.detached = true;
                        }
                    } else if let Some(pending) = self.assistant_panel.pending_query.take() {
                        self.assistant_panel
                            .transcripts
                            .entry(thread_id.clone())
                            .or_default()
                            .push(
                                TranscriptEntry::new(
                                    Speaker::Activity,
                                    "Query approval cancelled because the turn ended".into(),
                                    Some(turn.id.clone()),
                                )
                                .with_detail(bound_text(&pending.sql).0),
                            );
                    }
                }
                self.assistant_command(AssistantCommand::Read(thread_id.clone()), cx);
                if !self.assistant_panel.open {
                    self.assistant_panel.unread = true;
                }
                if self.assistant.selected_thread.as_deref() == Some(&thread_id) {
                    self.assistant_panel.active_turn = None;
                    self.assistant_panel.target = None;
                }
                if let Some(position) = self
                    .assistant
                    .conversations
                    .iter()
                    .position(|conversation| conversation.thread_id == thread_id)
                {
                    let mut conversation = self.assistant.conversations.remove(position);
                    conversation.last_activity = unix_now_seconds();
                    self.assistant.conversations.insert(0, conversation);
                    self.sync_assistant_selectors(window, cx);
                    self.changed(cx);
                }
                if let Some(error) = error {
                    self.assistant_panel
                        .transcripts
                        .entry(thread_id)
                        .or_default()
                        .push(TranscriptEntry::new(Speaker::Error, error, Some(turn.id)));
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::TitleChanged { thread_id, title }) => {
                if let Some(conversation) = self
                    .assistant
                    .conversations
                    .iter_mut()
                    .find(|conversation| conversation.thread_id == thread_id)
                    && conversation.title_source != AssistantTitleSource::User
                {
                    conversation.title = title;
                    conversation.title_source = AssistantTitleSource::Codex;
                    self.changed(cx);
                }
                self.sync_assistant_selectors(window, cx);
            }
            AssistantServiceEvent::Harness(AssistantEvent::ToolCall(call)) => {
                self.handle_assistant_tool(call, window, cx)
            }
            AssistantServiceEvent::Harness(AssistantEvent::Other { method, params }) => {
                if method == "account/login/completed" {
                    if params.get("success").and_then(Value::as_bool) == Some(true) {
                        self.assistant_command(AssistantCommand::Refresh, cx);
                    } else {
                        self.assistant_panel.notice = Some(
                            params
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("Sign-in failed. Try again.")
                                .to_owned(),
                        );
                    }
                } else if method == "account/updated" {
                    self.assistant_command(AssistantCommand::Refresh, cx);
                }
            }
            AssistantServiceEvent::LoginUrl(url) => {
                cx.open_url(&url);
                self.assistant_panel.notice =
                    Some("Complete sign-in in your browser, then return to Qrow.".into());
            }
            AssistantServiceEvent::Renamed(id) => {
                if let Some((pending_id, title)) = self.assistant_panel.pending_rename.take()
                    && pending_id == id
                    && let Some(conversation) = self
                        .assistant
                        .conversations
                        .iter_mut()
                        .find(|conversation| conversation.thread_id == id)
                {
                    conversation.title = title;
                    conversation.title_source = AssistantTitleSource::User;
                    self.sync_assistant_selectors(window, cx);
                    self.changed(cx);
                }
            }
            AssistantServiceEvent::Deleted(id) => {
                self.assistant_panel.older_cursors.remove(&id);
                self.assistant_panel.loaded_cursors.remove(&id);
                self.assistant
                    .conversations
                    .retain(|conversation| conversation.thread_id != id);
                self.assistant_panel.transcripts.remove(&id);
                if self.assistant.selected_thread.as_deref() == Some(&id) {
                    self.assistant.selected_thread = self
                        .assistant
                        .conversations
                        .first()
                        .map(|conversation| conversation.thread_id.clone());
                    if let Some(next) = self.assistant.selected_thread.clone() {
                        self.assistant_command(AssistantCommand::Resume(next), cx);
                    } else {
                        self.create_assistant_conversation(cx);
                    }
                }
                self.sync_assistant_selectors(window, cx);
                self.changed(cx);
            }
            AssistantServiceEvent::Disconnected(error) => {
                self.assistant_panel.creating_conversation = false;
                self.assistant_panel.status = Status::Disconnected(error);
                self.assistant_panel.service = None;
                self.assistant_panel.active_turn = None;
                self.assistant_panel.target = None;
                self.assistant_panel.pending_query = None;
            }
            AssistantServiceEvent::Failed {
                operation,
                id,
                error,
            } => {
                if operation == Operation::Rename {
                    self.assistant_panel.pending_rename = None;
                }
                if operation == Operation::Create {
                    self.assistant_panel.creating_conversation = false;
                }
                if operation == Operation::ReadOlder {
                    self.assistant_panel.loading_older = false;
                    if let Some(thread) = id.as_ref()
                        && let Some(cursor) = self.assistant_panel.older_cursors.get(thread)
                        && let Some(loaded) = self.assistant_panel.loaded_cursors.get_mut(thread)
                    {
                        loaded.remove(cursor);
                        if loaded.is_empty() {
                            self.assistant_panel.loaded_cursors.remove(thread);
                        }
                    }
                }
                if matches!(operation, Operation::Start | Operation::Steer)
                    && id.as_deref() == self.assistant.selected_thread.as_deref()
                {
                    self.assistant_panel.active_turn = None;
                    self.assistant_panel.target = None;
                    self.assistant_panel.pending_query = None;
                }
                if operation == Operation::Answer {
                    self.assistant_panel.status = Status::Disconnected(error);
                    self.assistant_panel.service = None;
                    return;
                }
                if let Some(thread) = id
                    .filter(|_| {
                        matches!(
                            operation,
                            Operation::Start
                                | Operation::Steer
                                | Operation::Resume
                                | Operation::Read
                                | Operation::ReadOlder
                                | Operation::Delete
                                | Operation::Rename
                        )
                    })
                    .or_else(|| self.assistant.selected_thread.clone())
                {
                    self.assistant_panel
                        .transcripts
                        .entry(thread)
                        .or_default()
                        .push(TranscriptEntry::new(Speaker::Error, error, None));
                } else {
                    self.assistant_panel.status = Status::Disconnected(error);
                }
            }
            _ => {}
        }
    }

    fn assistant_thread_list(
        &self,
        narrow: bool,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut conversations: Vec<_> = self.assistant.conversations.iter().collect();
        conversations.sort_by_key(|conversation| std::cmp::Reverse(conversation.last_activity));
        let search = self
            .assistant_panel
            .thread_search
            .read(cx)
            .value()
            .to_lowercase();
        v_flex()
            .w(if narrow { width } else { self.ui_px(230.) })
            .max_w_full()
            .h_full()
            .flex_shrink_0()
            .bg(cx.theme().sidebar)
            .border_l_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .h(self.ui_px(36.))
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .when(narrow, |row| {
                        row.child(
                            Button::new("assistant-back-to-thread")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowLeft)
                                .accessibility_label("Back to conversation")
                                .tooltip("Back to conversation")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.assistant_panel.thread_list_override = Some(false);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Input::new(&self.assistant_panel.thread_search)
                            .flex_1()
                            .min_w_0()
                            .aria_label("Search conversations"),
                    ),
            )
            .child(
                v_flex()
                    .id("assistant-thread-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .children(
                        conversations
                            .into_iter()
                            .filter(|conversation| {
                                self.conversation_label(conversation)
                                    .to_lowercase()
                                    .contains(&search)
                            })
                            .map(|conversation| {
                                let id = conversation.thread_id.clone();
                                let selected =
                                    self.assistant.selected_thread.as_deref() == Some(&id);
                                let label = self.conversation_label(conversation);
                                Button::new(format!("assistant-thread-{id}"))
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h(self.ui_px(48.))
                                    .justify_start()
                                    .accessibility_label(format!("Open conversation: {label}"))
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_0p5()
                                            .child(div().min_w_0().truncate().child(label))
                                            .child(
                                                div()
                                                    .text_xs()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child(conversation_age(
                                                        conversation.last_activity,
                                                        unix_now_seconds(),
                                                    )),
                                            ),
                                    )
                                    .selected(selected)
                                    .when(selected, |button| {
                                        button
                                            .bg(cx.theme().sidebar_accent)
                                            .text_color(cx.theme().sidebar_accent_foreground)
                                    })
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.select_assistant_thread(&id, window, cx);
                                    }))
                            }),
                    ),
            )
    }

    pub(super) fn assistant_panel(
        &self,
        width: Pixels,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let narrow = width < self.ui_px(600.);
        let show_threads = show_thread_list(narrow, self.assistant_panel.thread_list_override);
        let action_size = self.ui_px(28.);
        let status = match &self.assistant_panel.status {
            Status::Idle => "Open the assistant to start Codex".to_owned(),
            Status::Starting => "Starting Codex…".to_owned(),
            Status::SignInRequired => {
                "Sign in to Codex with ChatGPT to use your subscription.".to_owned()
            }
            Status::Ready => {
                if self
                    .assistant_panel
                    .pending_query
                    .as_ref()
                    .is_some_and(|query| query.started)
                {
                    "Query running…".into()
                } else if self.assistant_panel.active_turn.is_some() {
                    "Assistant is working…".into()
                } else if self
                    .assistant_panel
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| {
                        matches!(snapshot.account().kind(), AccountKind::ApiKey)
                    })
                {
                    "Ready · API-key billing applies".into()
                } else {
                    "Ready".into()
                }
            }
            Status::Disconnected(error) => error.clone(),
        };
        let selected = self.assistant.selected_thread.as_deref().unwrap_or("");
        let entries = self.assistant_panel.transcripts.get(selected);
        let mode_is_run = self.assistant.conversations.iter().any(|conversation| {
            conversation.thread_id == selected
                && conversation.execution_mode
                    == qrow::model::AssistantExecutionMode::RunAutomatically
        });
        let model = self.assistant_panel.snapshot.as_ref().and_then(|snapshot| {
            self.settings
                .assistant
                .model
                .as_deref()
                .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
                .or_else(|| snapshot.models().first())
        });
        let model_label = self
            .assistant_panel
            .model_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        let reasoning_label = self
            .assistant_panel
            .reasoning_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
        let tier_label = self
            .assistant_panel
            .tier_select
            .read(cx)
            .selected_value()
            .cloned()
            .unwrap_or_default();
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
        let [show_model_label, show_reasoning_label, show_tier_label] =
            assistant_selector_labels(
                (conversation_width - 196.).max(0.),
                [model_width, reasoning_width, tier_width],
            );
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
                    .pl_2()
                    .pr_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("assistant-toggle-threads")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .icon(IconName::PanelRight)
                            .accessibility_label("Toggle conversation list")
                            .tooltip("Toggle conversation list")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.assistant_panel.thread_list_override = Some(!show_threads);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .child(self.assistant.conversations.iter()
                                .find(|conversation| Some(&conversation.thread_id) == self.assistant.selected_thread.as_ref())
                                .map_or_else(|| "New conversation".to_owned(), |conversation| conversation.title.clone())),
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
                            .disabled(
                                !matches!(self.assistant_panel.status, Status::Ready)
                                    || self.assistant_panel.creating_conversation,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.create_assistant_conversation(cx);
                            })),
                    )
                    .child(
                        Button::new("assistant-conversation-menu")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .icon(IconName::Ellipsis)
                            .accessibility_label("Conversation actions")
                            .tooltip("Conversation actions")
                            .disabled(self.assistant.selected_thread.is_none() || self.assistant_panel.active_turn.is_some())
                            .dropdown_menu({
                                let rename = std::rc::Rc::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.begin_assistant_rename(window, cx);
                                }));
                                let delete = std::rc::Rc::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                                    this.confirm_assistant_delete(window, cx);
                                }));
                                move |menu, _, _| menu
                                    .item(PopupMenuItem::new("Rename…").on_click({let rename = rename.clone(); move |event, window, cx| rename(event, window, cx)}))
                                    .item(PopupMenuItem::new("Delete…").on_click({let delete = delete.clone(); move |event, window, cx| delete(event, window, cx)}))
                            }),
                    )
            )
            .when_some(
                self.assistant_panel.rename_input.as_ref(),
                |panel, input| {
                    panel.child(
                        h_flex()
                            .px_3()
                            .py_2()
                            .gap_2()
                            .child(Input::new(input).flex_1().aria_label("Conversation name"))
                            .child(
                                Button::new("assistant-save-rename")
                                    .small()
                                    .label("Save")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| {
                                            this.save_assistant_rename(cx)
                                        }),
                                    ),
                            )
                            .child(
                                Button::new("assistant-cancel-rename")
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.assistant_panel.rename_input = None;
                                        cx.notify();
                                    })),
                            ),
                    )
                },
            )
            .when(self.assistant_panel.active_turn.is_some() || matches!(self.assistant_panel.status, Status::Disconnected(_)), |panel| panel.child(
                h_flex()
                    .flex_shrink_0()
                    .px_3()
                    .py_1()
                    .justify_end()
                    .when(self.assistant_panel.active_turn.is_some(), |el| {
                        el.child(
                            Button::new("assistant-stop")
                                .small()
                                .label("Stop")
                                .on_click(cx.listener(|this, _, _, cx| this.stop_assistant(cx))),
                        )
                    })
                    .when(
                        matches!(self.assistant_panel.status, Status::Disconnected(_)),
                        |el| {
                            el.child(
                                Button::new("assistant-reconnect")
                                    .small()
                                    .label("Reconnect")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.reconnect_assistant(cx)),
                                    ),
                            )
                        },
                    ),
            ))
            .when(
                matches!(self.assistant_panel.status, Status::SignInRequired),
                |panel| {
                    panel.child(
                        Button::new("assistant-sign-in")
                            .label("Sign in with ChatGPT")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.assistant_command(AssistantCommand::Login, cx);
                            })),
                    )
                },
            )
            .child(
                div().relative().flex_1().min_h_0().child(v_flex()
                    .id("assistant-transcript")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.assistant_panel.scroll)
                    .px_3()
                    .py_3()
                    .gap_3()
                    .when(self.assistant_panel.older_cursors.contains_key(selected), |transcript| transcript.child(
                        Button::new("assistant-load-older").ghost().small().label("Load older messages")
                            .disabled(self.assistant_panel.loading_older || self.assistant_panel.active_turn.is_some() || self.assistant_panel.pending_query.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.load_older_assistant_messages(cx)))))
                    .children(
                        entries
                            .into_iter()
                            .flatten()
                            .enumerate()
                            .map(|(index, entry)| {
                                let content = if entry.speaker == Speaker::Assistant {
                                    let mut table_style = StyleRefinement::default();
                                    table_style.overflow.x = Some(Overflow::Scroll);
                                    TextView::markdown(
                                        format!("assistant-markdown-{}", entry.id),
                                        entry.text.clone(),
                                    )
                                    .style(TextViewStyle::default().table(table_style))
                                    .min_w_0()
                                    .max_w_full()
                                    .into_any_element()
                                } else {
                                    SelectableText::new("message", entry.text.clone())
                                        .document_order(index as u64 * 2)
                                        .into_any_element()
                                };
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
                                Message::new().alignment(alignment).content(
                                    MessageContent::new().bubble(
                                        Bubble::new().with_variant(variant).child(
                                        div()
                                            .id(format!("assistant-entry-{}", entry.id))
                                            .role(Role::Paragraph)
                                            .aria_label(format!(
                                                "{}: {}",
                                                match entry.speaker {
                                                    Speaker::User => "You",
                                                    Speaker::Assistant => "Assistant",
                                                    Speaker::Activity => "Assistant activity",
                                                    Speaker::Error => "Assistant error",
                                                },
                                                entry.text
                                            ))
                                            .whitespace_normal()
                                            .when(entry.speaker == Speaker::Activity, |bubble| bubble.font_weight(FontWeight::MEDIUM))
                                            .child(content)
                                            .when_some(entry.detail.as_ref(), |bubble, detail| {
                                                let thread = selected.to_owned();
                                                bubble.child(Button::new(format!("assistant-detail-{index}"))
                                                    .small().label(if entry.expanded { "Hide details" } else { "Details" })
                                                    .on_click(cx.listener(move |this, _, _, cx| {
                                                        if let Some(entry) = this.assistant_panel.transcripts.get_mut(&thread)
                                                            .and_then(|entries| entries.get_mut(index)) {
                                                            entry.expanded = !entry.expanded;
                                                            cx.notify();
                                                        }
                                                    })))
                                                    .when(entry.expanded, |bubble| bubble.child(
                                                        div().id(format!("assistant-detail-content-{index}"))
                                                            .max_h_40().max_w_full().overflow_y_scroll().overflow_x_scroll()
                                                            .font_family(self.settings.editor_font_family.clone())
                                                            .text_xs()
                                                            .child(SelectableText::new("detail", detail.clone()).document_order(index as u64 * 2 + 1))
                                                    ))
                                            })),
                                    ),
                                )
                            }),
                    ))
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
                            .accessibility_label("Jump to latest")
                            .tooltip("Jump to latest")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.assistant_panel.scroll.scroll_to_bottom();
                                cx.notify();
                            })),
                    )
                },
            ))
            .when_some(self.assistant_panel.notice.as_ref(), |panel, notice| {
                panel.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(notice.clone()),
                )
            })
            .when_some(
                self.assistant_panel
                    .pending_query
                    .as_ref()
                    .filter(|pending| !pending.approved),
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
                            .role(Role::Alert)
                            .aria_label(format!(
                                "Assistant query approval: {tab_title}, {connection}. {}",
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
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.approve_assistant_query(window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("assistant-cancel-query")
                                            .small()
                                            .label("Cancel")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_assistant_approval(cx)
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
                    .when(status != "Ready", |composer| composer.child(
                        div()
                            .id("assistant-status")
                            .role(Role::Status)
                            .aria_label(status.clone())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    ))
                    .child(
                        div().key_context("AssistantComposer").child(
                            Textarea::new(&self.assistant_panel.composer)
                                .h_20()
                                .w_full()
                                .aria_label("Assistant message")),
                    )
                    .child(
                        h_flex().min_w_0().gap_1()
                            .child(
                                h_flex()
                                    .h(action_size)
                                    .min_w_0()
                                    .gap_1()
                                    .when(model.is_some(), |row| row.child(
                                        Button::new("assistant-model")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::Cpu)
                                            .accessibility_label(format!("Assistant model: {model_label}"))
                                            .tooltip(format!("Model: {model_label}"))
                                            .when(show_model_label, |button| button.label(model_label.clone()).dropdown_caret(true))
                                            .dropdown_menu({
                                                let assistant_entity = assistant_entity.clone();
                                                move |menu, _, _| model_options.iter().fold(menu, |menu, (label, selected)| {
                                                    let label = label.clone();
                                                    let assistant_entity = assistant_entity.clone();
                                                    menu.item(PopupMenuItem::new(label.clone()).checked(*selected).on_click(
                                                        move |_, window, cx| {
                                                            assistant_entity.update(cx, |this, cx| {
                                                                this.select_assistant_model(&label, window, cx);
                                                            });
                                                        },
                                                    ))
                                                })
                                            })))
                                    .when(model.is_some_and(|model| !model.reasoning_efforts().is_empty()), |row| row.child(
                                        Button::new("assistant-reasoning")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::Asterisk)
                                            .accessibility_label(format!("Assistant reasoning: {reasoning_label}"))
                                            .tooltip(format!("Reasoning: {reasoning_label}"))
                                            .when(show_reasoning_label, |button| button.label(reasoning_label.clone()).dropdown_caret(true))
                                            .dropdown_menu({
                                                let assistant_entity = assistant_entity.clone();
                                                move |menu, _, _| reasoning_options.iter().fold(menu, |menu, (label, selected)| {
                                                    let label = label.clone();
                                                    let assistant_entity = assistant_entity.clone();
                                                    menu.item(PopupMenuItem::new(label.clone()).checked(*selected).on_click(
                                                        move |_, window, cx| {
                                                            assistant_entity.update(cx, |this, cx| {
                                                                this.select_assistant_reasoning(&label, window, cx);
                                                            });
                                                        },
                                                    ))
                                                })
                                            })))
                                    .when(model.is_some_and(|model| !model.service_tiers().is_empty()), |row| row.child(
                                        Button::new("assistant-tier")
                                            .ghost().small().compact()
                                            .icon(AssetIconName::BatteryCharging)
                                            .accessibility_label(format!("Assistant service tier: {tier_label}"))
                                            .tooltip(format!("Service tier: {tier_label}"))
                                            .when(show_tier_label, |button| button.label(tier_label.clone()).dropdown_caret(true))
                                            .dropdown_menu({
                                                let assistant_entity = assistant_entity.clone();
                                                move |menu, _, _| tier_options.iter().fold(menu, |menu, (label, selected)| {
                                                    let label = label.clone();
                                                    let assistant_entity = assistant_entity.clone();
                                                    menu.item(PopupMenuItem::new(label.clone()).checked(*selected).on_click(
                                                        move |_, window, cx| {
                                                            assistant_entity.update(cx, |this, cx| {
                                                                this.select_assistant_tier(&label, window, cx);
                                                            });
                                                        },
                                                    ))
                                                })
                                            }))),
                            )
                            .child(div().flex_1())
                            .child(
                                DropdownButton::new("assistant-send-mode")
                                    .primary()
                                    .small()
                                    .disabled(
                                        !matches!(self.assistant_panel.status, Status::Ready)
                                            || self.assistant.selected_thread.is_none(),
                                    )
                                    .button(
                                        Button::new("assistant-send")
                                            .icon(AssetIconName::Send)
                                            .label(if mode_is_run { "Send · Run" } else { "Send · Ask" })
                                            .tooltip(if mode_is_run {
                                                "Send message · run assistant queries automatically"
                                            } else {
                                                "Send message · ask before running assistant queries"
                                            })
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.send_assistant(window, cx)
                                            })),
                                    )
                                    .dropdown_menu({
                                        let ask = std::rc::Rc::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            if this.assistant.conversations.iter().any(|conversation| {
                                                Some(&conversation.thread_id) == this.assistant.selected_thread.as_ref()
                                                    && conversation.execution_mode == qrow::model::AssistantExecutionMode::RunAutomatically
                                            }) {
                                                this.toggle_assistant_mode(window, cx);
                                            }
                                        }));
                                        let run = std::rc::Rc::new(cx.listener(|this, _: &ClickEvent, window, cx| {
                                            if this.assistant.conversations.iter().any(|conversation| {
                                                Some(&conversation.thread_id) == this.assistant.selected_thread.as_ref()
                                                    && conversation.execution_mode == qrow::model::AssistantExecutionMode::AskBeforeRunning
                                            }) {
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
                            ),
                    ),
            )))
            .when(show_threads, |panel| panel.child(self.assistant_thread_list(narrow, width, cx)))
    }
}
