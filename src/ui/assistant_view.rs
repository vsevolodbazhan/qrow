use super::*;
use crate::{
    assistant::{
        AccountKind, AssistantEvent, HarnessSnapshot, HistoryTurn, MAX_MESSAGE_BYTES, TitleRequest,
        ToolCall, TurnRequest,
        broker::{
            ActionTarget, ConnectionContext, ConnectionState, MAX_CONTEXT_SQL_BYTES, QueryState,
            ResultSummary, SelectedTabContext, TabSummary, WorkspaceContext, bound_text,
            context_statement_ranges, sql_window,
        },
        history_item_text,
        service::{
            Command as AssistantCommand, Event as AssistantServiceEvent, Operation, Service,
        },
        tools,
    },
    model::{
        AssistantConversation, AssistantExecutionMode, AssistantTitleSource,
        MAX_ASSISTANT_CONVERSATION_TITLE,
    },
};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{
    Icon, Selectable,
    alert::Alert,
    bubble::{Bubble, BubbleContent, BubbleVariant},
    button::{ButtonRounded, DropdownButton},
    collapsible::Collapsible,
    empty::{
        Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyMediaVariant,
        EmptyTitle,
    },
    h_flex,
    input::{Textarea, TextareaState},
    menu::DropdownMenu,
    message::{Message, MessageAlignment, MessageContent},
    shimmer::ShimmerText,
    spinner::Spinner,
    text::{TextView, TextViewStyle},
    v_flex,
};
use serde_json::{Value, json};
use std::ops::Range;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::PathBuf,
    rc::Rc,
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

/// A narrow pane swaps the opened list for the selected conversation. A wide
/// pane keeps the list next to it.
fn thread_list_after_selection(override_visibility: Option<bool>) -> Option<bool> {
    match override_visibility {
        Some(true) => None,
        other => other,
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AssistantNotice {
    Info(String),
    Warning(String),
    Error(String),
}

impl AssistantNotice {
    fn info(message: impl Into<String>) -> Self {
        Self::Info(message.into())
    }

    fn warning(message: impl Into<String>) -> Self {
        Self::Warning(message.into())
    }

    fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }
}

/// Progress of a browser sign-in. Codex owns the account and the sign-in page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) enum SignIn {
    #[default]
    Idle,
    /// Qrow asked Codex for the sign-in page.
    Starting,
    /// The sign-in page is open in the browser.
    Waiting {
        login_id: String,
        url: String,
    },
    Failed(String),
}

impl SignIn {
    /// Applies `account/login/completed`. A failure applies only to the sign-in
    /// on screen, because Codex also reports a sign-in that you cancelled or
    /// replaced.
    fn complete(&mut self, login_id: Option<&str>, success: bool, error: Option<&str>) {
        if success {
            *self = Self::Idle;
        } else if let Self::Waiting {
            login_id: current, ..
        } = self
            && login_id.is_none_or(|id| id == current)
        {
            *self = Self::Failed(error.unwrap_or("Codex did not give a reason.").to_owned());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::WORKSPACE_CONTEXT_SEPARATOR;

    #[::core::prelude::v1::test]
    fn sign_in_failures_apply_only_to_the_sign_in_on_screen() {
        let waiting = SignIn::Waiting {
            login_id: "login-2".into(),
            url: "https://example.invalid".into(),
        };
        let mut sign_in = waiting.clone();
        sign_in.complete(Some("login-1"), false, Some("Login cancelled"));
        assert_eq!(sign_in, waiting);
        sign_in.complete(Some("login-2"), false, Some("Login cancelled"));
        assert_eq!(sign_in, SignIn::Failed("Login cancelled".into()));

        // A sign-in that you cancelled in Qrow reports a failure later.
        let mut sign_in = SignIn::Idle;
        sign_in.complete(Some("login-2"), false, Some("Login cancelled"));
        assert_eq!(sign_in, SignIn::Idle);

        let mut sign_in = SignIn::Failed("Login cancelled".into());
        sign_in.complete(None, true, None);
        assert_eq!(sign_in, SignIn::Idle);
    }

    #[::core::prelude::v1::test]
    fn tool_cards_keep_the_tool_name_and_show_the_state_separately() {
        let tool = |kind, state| ToolActivity {
            kind,
            target: Some("Clicks".into()),
            state,
        };
        let mut entry = TranscriptEntry::tool(
            tool(ToolKind::RunQuery, ToolState::Running("Preparing".into())),
            "turn-1".into(),
        );
        assert!(!entry.expanded);
        assert_eq!(entry.text, "Run query · Preparing");
        entry.set_tool_state(ToolState::Done(Some("3 rows · 0.20 s".into())));
        assert_eq!(entry.text, "Run query · 3 rows · 0.20 s");
        entry.set_tool_state(ToolState::Failed);
        assert_eq!(entry.text, "Run query · Failed");
        let edit = TranscriptEntry::tool(
            tool(
                ToolKind::from_name("edit_selected_tab_sql"),
                ToolState::Done(None),
            ),
            "turn-1".into(),
        );
        assert_eq!(edit.text, "Edit query");
        assert_eq!(ToolKind::from_name("unknown_tool"), ToolKind::Other);
        assert!(
            TranscriptEntry::new(Speaker::Error, "Failed".into(), None)
                .tool
                .is_none()
        );
    }

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
    fn history_merge_keeps_steered_message_visible_without_workspace_context() {
        let mut entries = vec![
            TranscriptEntry::new(Speaker::User, "First".into(), Some("turn-1".into())),
            TranscriptEntry::new(Speaker::User, "Follow-up".into(), Some("turn-1".into())),
        ];
        let steered = format!(
            "Follow-up{WORKSPACE_CONTEXT_SEPARATOR}{}",
            json!({"version":1,"connections":[],"tabs":[],"selected_tab":null})
        );
        merge_history(
            &mut entries,
            vec![HistoryTurn {
                id: "turn-1".into(),
                status: "completed".into(),
                items: vec![
                    json!({"type":"userMessage","content":[{"type":"text","text":"First"}]}),
                    json!({"type":"userMessage","content":[{"type":"text","text":steered}]}),
                ],
            }],
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].text, "Follow-up");
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

    fn pending_query(approved: bool) -> PendingQuery {
        PendingQuery {
            call: ToolCall {
                request_id: json!(1),
                call_id: "call".into(),
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                name: "run_selected_tab_query".into(),
                arguments: Value::Null,
            },
            tab_id: Uuid::new_v4(),
            revision: 0,
            sql: "SELECT 1".into(),
            approved,
            started: false,
            kind: PendingQueryKind::Run,
            activity_index: None,
            detached: false,
            first_row: 0,
        }
    }

    #[::core::prelude::v1::test]
    fn conversation_state_shows_approval_before_work_and_unread_replies() {
        let mut run = ThreadRun::default();
        assert_eq!(ThreadStatus::of(&run), ThreadStatus::Idle);
        run.unread = Some(Unread::Reply);
        assert_eq!(ThreadStatus::of(&run), ThreadStatus::Ready);
        run.unread = Some(Unread::Error);
        assert_eq!(ThreadStatus::of(&run), ThreadStatus::Failed);
        run.pending_reply = true;
        assert_eq!(ThreadStatus::of(&run), ThreadStatus::Working);
        run.active_turn = Some("turn".into());
        run.pending_query = Some(pending_query(true));
        assert_eq!(ThreadStatus::of(&run), ThreadStatus::Working);
        run.pending_query = Some(pending_query(false));
        assert_eq!(ThreadStatus::of(&run), ThreadStatus::Approval);
        assert!(ThreadStatus::Approval.busy() && ThreadStatus::Working.busy());
        assert!(!ThreadStatus::Ready.busy() && !ThreadStatus::Failed.busy());
    }

    #[::core::prelude::v1::test]
    fn the_assistant_toggle_shows_the_most_urgent_conversation_state() {
        let urgent = [
            ThreadStatus::Working,
            ThreadStatus::Idle,
            ThreadStatus::Ready,
            ThreadStatus::Approval,
            ThreadStatus::Failed,
        ]
        .into_iter()
        .fold(ThreadStatus::Idle, Ord::max);
        assert_eq!(urgent, ThreadStatus::Approval);
        assert!(ThreadStatus::Failed > ThreadStatus::Ready);
        assert!(ThreadStatus::Ready > ThreadStatus::Working);
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
        assert_eq!(
            assistant_selector_labels(228., widths),
            [true, false, false]
        );
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

/// A Qrow tool that the assistant can call. The name stays the same while the
/// call runs and after it ends. The card state shows the progress and outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ToolKind {
    Workspace,
    ReadQuery,
    AppendQuery,
    EditQuery,
    RunQuery,
    CancelQuery,
    QueryStatus,
    ReadResults,
    FetchRows,
    ReadLogs,
    Other,
}

impl ToolKind {
    pub fn from_name(name: &str) -> Self {
        match name {
            "get_workspace_context" => Self::Workspace,
            "read_tab_sql" => Self::ReadQuery,
            "append_selected_tab_sql" => Self::AppendQuery,
            "edit_selected_tab_sql" => Self::EditQuery,
            "run_selected_tab_query" => Self::RunQuery,
            "cancel_selected_tab_query" => Self::CancelQuery,
            "get_query_status" => Self::QueryStatus,
            "read_results" => Self::ReadResults,
            "fetch_more_results" => Self::FetchRows,
            "read_query_logs" => Self::ReadLogs,
            _ => Self::Other,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::Workspace => "Read workspace",
            Self::ReadQuery => "Read query",
            Self::AppendQuery => "Append query",
            Self::EditQuery => "Edit query",
            Self::RunQuery => "Run query",
            Self::CancelQuery => "Cancel query",
            Self::QueryStatus => "Check query status",
            Self::ReadResults => "Read results",
            Self::FetchRows => "Fetch more rows",
            Self::ReadLogs => "Read logs",
            Self::Other => "Use tool",
        }
    }

    fn icon(self) -> AssetIconName {
        match self {
            Self::Workspace => AssetIconName::LayoutDashboard,
            Self::ReadQuery => AssetIconName::FileText,
            Self::AppendQuery => AssetIconName::ListPlus,
            Self::EditQuery => AssetIconName::Pencil,
            Self::RunQuery => AssetIconName::Play,
            Self::CancelQuery => AssetIconName::CircleStop,
            Self::QueryStatus => AssetIconName::Activity,
            Self::ReadResults => AssetIconName::Table,
            Self::FetchRows => AssetIconName::ListPlus,
            Self::ReadLogs => AssetIconName::ScrollText,
            Self::Other => AssetIconName::SquareTerminal,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ToolState {
    /// The call is in progress. The text names the current step.
    Running(String),
    /// The call succeeded. The optional text is a short outcome.
    Done(Option<String>),
    Failed,
    /// Qrow did not run the call, for example because the user declined it.
    Cancelled,
}

impl ToolState {
    fn label(&self) -> Option<&str> {
        match self {
            Self::Running(step) => Some(step),
            Self::Done(outcome) => outcome.as_deref(),
            Self::Failed => Some("Failed"),
            Self::Cancelled => Some("Cancelled"),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ToolActivity {
    pub kind: ToolKind,
    /// The query tab the call used. The expanded card shows it.
    pub target: Option<String>,
    pub state: ToolState,
}

impl ToolActivity {
    /// Matches the collapsed card header: the tool name and its state.
    fn label(&self) -> String {
        match self.state.label() {
            Some(state) => format!("{} · {state}", self.kind.title()),
            None => self.kind.title().to_owned(),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct TranscriptEntry {
    id: Uuid,
    pub speaker: Speaker,
    pub text: String,
    pub turn_id: Option<String>,
    pub tool: Option<ToolActivity>,
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
            tool: None,
            detail: None,
            expanded: false,
        }
    }

    pub fn tool(tool: ToolActivity, turn_id: String) -> Self {
        let mut entry = Self::new(Speaker::Activity, tool.label(), Some(turn_id));
        entry.tool = Some(tool);
        entry
    }

    pub fn with_detail(mut self, detail: String) -> Self {
        self.detail = Some(detail);
        self
    }

    /// Replaces the tool state and keeps the accessible text in step with it.
    pub fn set_tool_state(&mut self, state: ToolState) {
        if let Some(tool) = &mut self.tool {
            tool.state = state;
            self.text = tool.label();
        }
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
    /// The first result row that this request adds, for the rows in its result.
    pub first_row: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PendingQueryKind {
    Run,
    Fetch,
}

/// A turn end that the user has not seen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Unread {
    Reply,
    Error,
}

/// The live state of one conversation. Several conversations can have a turn
/// at the same time.
pub(super) struct AppendedQuery {
    pub turn_id: String,
    pub tab_id: Uuid,
    pub connection_id: Option<Uuid>,
    pub revision: u64,
    pub selected_range: Range<usize>,
}

#[derive(Default)]
pub(super) struct ThreadRun {
    pub active_turn: Option<String>,
    /// Codex owes a reply from the send until the turn ends.
    pub pending_reply: bool,
    pub target: Option<ActionTarget>,
    pub appended_query: Option<AppendedQuery>,
    pub pending_query: Option<PendingQuery>,
    pub unread: Option<Unread>,
    pub loading_older: bool,
    /// The texts of the sent messages that Codex has not started or steered
    /// yet, oldest first. A failed send puts its text back in the message field.
    pub sent_messages: VecDeque<String>,
}

/// The conversation state that the thread list, the tab strip, and the
/// assistant toggle show. A higher state is more urgent.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum ThreadStatus {
    Idle,
    Working,
    Ready,
    Failed,
    Approval,
}

impl ThreadStatus {
    fn of(run: &ThreadRun) -> Self {
        if run
            .pending_query
            .as_ref()
            .is_some_and(|pending| !pending.approved)
        {
            Self::Approval
        } else if run.active_turn.is_some() || run.pending_reply {
            Self::Working
        } else {
            match run.unread {
                Some(Unread::Error) => Self::Failed,
                Some(Unread::Reply) => Self::Ready,
                None => Self::Idle,
            }
        }
    }

    /// A conversation in this state keeps its tab open and on its connection.
    pub fn busy(self) -> bool {
        matches!(self, Self::Working | Self::Approval)
    }

    /// Names the state after a tab or conversation name.
    pub fn accessible_suffix(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Working => ", assistant working",
            Self::Ready => ", assistant reply ready",
            Self::Failed => ", assistant reply failed",
            Self::Approval => ", assistant waiting for approval",
        }
    }
}

/// The first message from a tab without a conversation. Qrow sends it when
/// Codex has created the conversation.
pub(super) struct FirstMessage {
    pub tab_id: Uuid,
    pub text: String,
    pub mode: AssistantExecutionMode,
    /// Shows the message until the conversation has it.
    pub entry: TranscriptEntry,
}

/// A conversation, or a tab that does not have one yet.
#[derive(Clone, Debug)]
enum ModeTarget {
    Thread(String),
    Tab(Uuid),
}

/// The open rename dialog for one conversation.
pub(super) struct ConversationEditor {
    thread_id: String,
    title: Entity<InputState>,
    error: Option<String>,
}

/// The time that quit waits for Codex to stop, including a forced stop.
const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Qrow stops Codex when the pane stays closed for this time without Codex
/// work. The next open starts Codex again.
pub const CODEX_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

const CONVERSATION_RENAME: tab_view::RenameDialog = tab_view::RenameDialog {
    key: "conversation",
    label: "Conversation Name",
    tooltip: "Rename Conversation · ⌘Enter",
    form: |this| {
        this.assistant_panel
            .rename_form
            .as_ref()
            .map(|form| (&form.title, form.error.clone()))
    },
    submit: Qrow::save_assistant_rename,
    clear: |this| this.assistant_panel.rename_form = None,
};

type MenuAction = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The actions for one conversation. The pane header menu and the thread list
/// context menu use the same items and layout.
struct ConversationMenu {
    busy: bool,
    can_regenerate: bool,
    rename: MenuAction,
    regenerate: MenuAction,
    delete: MenuAction,
}

impl Qrow {
    /// The menu of a thread list row. Qrow owns it like the tab menu; GPUI
    /// Kit's `context_menu` keeps each dismissed menu alive.
    fn open_thread_menu(
        &mut self,
        thread: &str,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.conversation_menu(thread, cx);
        self.open_context_menu(position, move |popup, _, _| menu.build(popup), window, cx);
    }
}

impl ConversationMenu {
    fn build(&self, menu: PopupMenu) -> PopupMenu {
        let item = |label: &'static str, action: &MenuAction, disabled: bool| {
            let action = action.clone();
            PopupMenuItem::new(label)
                .on_click(move |event, window, cx| action(event, window, cx))
                .disabled(disabled)
        };
        menu.item(item("Rename…", &self.rename, self.busy))
            .item(item(
                "Regenerate Title",
                &self.regenerate,
                !self.can_regenerate,
            ))
            .separator()
            .item(item("Delete…", &self.delete, self.busy))
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum ComposerTarget {
    Tab(Uuid),
    Detached(String),
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
    /// A closed conversation shown without opening a query tab.
    browsed_thread: Option<String>,
    pub transcripts: BTreeMap<String, Vec<TranscriptEntry>>,
    pub older_cursors: BTreeMap<String, String>,
    pub loaded_cursors: BTreeMap<String, BTreeSet<String>>,
    pub runs: BTreeMap<String, ThreadRun>,
    /// Conversations that the current Codex process has loaded.
    pub loaded_threads: BTreeSet<String>,
    /// First messages in the order of their `Create` commands.
    pub first_messages: VecDeque<FirstMessage>,
    /// Unsent messages outside the current composer target.
    pub drafts: BTreeMap<ComposerTarget, String>,
    /// The tab or closed conversation whose draft the composer shows.
    pub composer_target: Option<ComposerTarget>,
    /// Query modes of tabs that do not have a conversation yet.
    pub draft_modes: BTreeMap<Uuid, AssistantExecutionMode>,
    /// Tabs opened by New Conversation before their first message creates a thread.
    pub new_conversation_tabs: BTreeSet<Uuid>,
    pub scroll: ScrollHandle,
    pub resizing: Option<(Point<Pixels>, Pixels)>,
    pub notice: Option<AssistantNotice>,
    pub sign_in: SignIn,
    pub previous_focus: Option<FocusHandle>,
    pub rename_form: Option<ConversationEditor>,
    pub pending_rename: Option<(String, String)>,
    /// Conversations with a title request from the user. The new title
    /// replaces a title that the user set.
    pub regenerating_titles: BTreeSet<String>,
    /// Conversations with an active Codex title request.
    pub pending_titles: BTreeSet<String>,
    /// Conversations that wait for their history before Qrow requests a title.
    pub title_history_reads: BTreeSet<String>,
    /// Conversations without a turn. Qrow does not save them.
    pub unstarted_threads: BTreeSet<String>,
    /// Stops an idle Codex while the pane is closed.
    idle_stop: Option<Task<()>>,
    /// The last Codex command or event, or the time that the pane closed.
    idle_since: Instant,
}

impl AssistantPanelState {
    pub fn new(window: &mut Window, cx: &mut Context<Qrow>) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .placeholder("Ask about your data or describe a query…")
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
            browsed_thread: None,
            transcripts: BTreeMap::new(),
            older_cursors: BTreeMap::new(),
            loaded_cursors: BTreeMap::new(),
            runs: BTreeMap::new(),
            loaded_threads: BTreeSet::new(),
            first_messages: VecDeque::new(),
            drafts: BTreeMap::new(),
            composer_target: None,
            draft_modes: BTreeMap::new(),
            new_conversation_tabs: BTreeSet::new(),
            scroll: ScrollHandle::new(),
            resizing: None,
            notice: None,
            sign_in: SignIn::Idle,
            previous_focus: None,
            rename_form: None,
            pending_rename: None,
            regenerating_titles: BTreeSet::new(),
            pending_titles: BTreeSet::new(),
            title_history_reads: BTreeSet::new(),
            unstarted_threads: BTreeSet::new(),
            idle_stop: None,
            idle_since: cx.background_executor().now(),
        }
    }

    /// Stops Codex in the background. The window does not wait.
    pub fn stop(&mut self) {
        self.idle_stop = None;
        if let Some(mut service) = self.service.take() {
            service.stop();
        }
    }

    /// Stops Codex at quit. Waits for it and for the background stops, so
    /// that no Codex process outlives Qrow.
    pub fn shutdown(&mut self) {
        let deadline = Instant::now() + QUIT_TIMEOUT;
        if let Some(mut service) = self.service.take() {
            let _ = service.shutdown_and_wait(QUIT_TIMEOUT);
        }
        Service::wait_for_background_stops(deadline);
    }

    /// Deletes the demo conversations and stops Codex at quit.
    pub fn shutdown_demo(&mut self, ids: Vec<String>) {
        let deadline = Instant::now() + QUIT_TIMEOUT;
        if let Some(mut service) = self.service.take()
            && let Err(error) = service.shutdown_and_delete(ids, QUIT_TIMEOUT)
        {
            eprintln!("Could not clean up demo assistant conversations: {error}");
        }
        Service::wait_for_background_stops(deadline);
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
    pub(super) fn active_tab_id(&self) -> Option<Uuid> {
        self.tabs.get(self.active).map(|tab| tab.saved.id)
    }

    /// The conversation shown in the pane, including a selected closed thread.
    pub(super) fn displayed_thread(&self) -> Option<String> {
        if let Some(thread) = self.assistant_panel.browsed_thread.as_ref()
            && self
                .assistant
                .conversation(thread)
                .is_some_and(|conversation| conversation.tab_id.is_none())
        {
            return Some(thread.clone());
        }
        self.active_tab_id()
            .and_then(|tab| self.assistant.conversation_for_tab(tab))
            .map(|conversation| conversation.thread_id.clone())
    }

    /// The index of a conversation's query tab.
    pub(super) fn thread_tab_index(&self, thread_id: &str) -> Option<usize> {
        let tab = self.assistant.conversation(thread_id)?.tab_id?;
        self.tabs
            .iter()
            .position(|candidate| candidate.saved.id == tab)
    }

    pub(super) fn thread_run(&self, thread_id: &str) -> Option<&ThreadRun> {
        self.assistant_panel.runs.get(thread_id)
    }

    pub(super) fn thread_run_mut(&mut self, thread_id: &str) -> &mut ThreadRun {
        self.assistant_panel
            .runs
            .entry(thread_id.to_owned())
            .or_default()
    }

    pub(super) fn thread_status(&self, thread_id: &str) -> ThreadStatus {
        self.thread_run(thread_id)
            .map_or(ThreadStatus::Idle, ThreadStatus::of)
    }

    /// The assistant state of a query tab, or `None` for a tab without a
    /// conversation.
    pub(super) fn tab_assistant_status(&self, tab_id: Uuid) -> Option<ThreadStatus> {
        if self
            .assistant_panel
            .first_messages
            .iter()
            .any(|first| first.tab_id == tab_id)
        {
            return Some(ThreadStatus::Working);
        }
        self.assistant
            .conversation_for_tab(tab_id)
            .map(|conversation| self.thread_status(&conversation.thread_id))
    }

    /// A tab whose conversation works or waits for approval cannot close or
    /// move.
    pub(super) fn assistant_tab_busy(&self, tab_id: Uuid) -> bool {
        self.tab_assistant_status(tab_id)
            .is_some_and(ThreadStatus::busy)
    }

    /// Whether a conversation has a turn that quitting would stop.
    pub(super) fn assistant_working(&self) -> bool {
        !self.assistant_panel.first_messages.is_empty()
            || self
                .assistant_panel
                .runs
                .values()
                .any(|run| run.active_turn.is_some() || run.pending_reply)
    }

    /// Whether Codex has work that a new Codex process would lose. A new
    /// process cannot resume a conversation without a turn, and it does not
    /// know the requests, tool calls, and sign-in of the old process.
    fn codex_has_work(&self) -> bool {
        let panel = &self.assistant_panel;
        self.assistant_working()
            || panel
                .runs
                .values()
                .any(|run| run.pending_query.is_some() || run.loading_older)
            || !panel.pending_titles.is_empty()
            || !panel.regenerating_titles.is_empty()
            || !panel.title_history_reads.is_empty()
            || panel.pending_rename.is_some()
            || matches!(panel.sign_in, SignIn::Starting | SignIn::Waiting { .. })
            || self
                .assistant
                .conversations
                .iter()
                .any(|conversation| panel.unstarted_threads.contains(&conversation.thread_id))
    }

    /// The most urgent state of all conversations, for the assistant toggle.
    pub(super) fn assistant_status(&self) -> ThreadStatus {
        let first = if self.assistant_panel.first_messages.is_empty() {
            ThreadStatus::Idle
        } else {
            ThreadStatus::Working
        };
        self.assistant_panel
            .runs
            .values()
            .map(ThreadStatus::of)
            .fold(first, Ord::max)
    }

    fn mode_target(&self) -> Option<ModeTarget> {
        self.displayed_thread()
            .map(ModeTarget::Thread)
            .or_else(|| self.active_tab_id().map(ModeTarget::Tab))
    }

    /// The query mode of the displayed conversation. A tab without a
    /// conversation uses the mode for its first message.
    pub(super) fn displayed_mode(&self) -> AssistantExecutionMode {
        match self.mode_target() {
            Some(ModeTarget::Thread(thread)) => self
                .assistant
                .conversation(&thread)
                .map(|conversation| conversation.execution_mode)
                .unwrap_or_default(),
            Some(ModeTarget::Tab(tab)) => self
                .assistant_panel
                .draft_modes
                .get(&tab)
                .copied()
                .unwrap_or(self.settings.assistant.default_execution_mode),
            None => self.settings.assistant.default_execution_mode,
        }
    }

    fn set_mode(
        &mut self,
        target: &ModeTarget,
        mode: AssistantExecutionMode,
        cx: &mut Context<Self>,
    ) {
        match target {
            ModeTarget::Thread(thread) => {
                if let Some(conversation) = self.assistant.conversation_mut(thread) {
                    conversation.execution_mode = mode;
                    self.changed(cx);
                }
            }
            ModeTarget::Tab(tab) => {
                self.assistant_panel.draft_modes.insert(*tab, mode);
                cx.notify();
            }
        }
    }

    /// Shows the conversation of the active tab after a tab change.
    pub(super) fn show_tab_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.assistant_panel.browsed_thread = None;
        let target = self.active_tab_id().map(ComposerTarget::Tab);
        self.show_assistant_composer(target, window, cx);
        if let Some(thread) = self.displayed_thread() {
            if self.assistant_panel.open {
                self.thread_run_mut(&thread).unread = None;
            }
            self.load_assistant_thread(&thread, cx);
        }
    }

    /// Keeps an unsent message with the tab or closed conversation that owns it.
    fn show_assistant_composer(
        &mut self,
        target: Option<ComposerTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.assistant_panel.composer_target == target {
            return;
        }
        if let Some(previous) = self.assistant_panel.composer_target.take()
            && match &previous {
                ComposerTarget::Tab(tab) => self.tabs.iter().any(|item| item.saved.id == *tab),
                ComposerTarget::Detached(thread) => self.assistant.conversation(thread).is_some(),
            }
        {
            let text = self.assistant_panel.composer.read(cx).value().to_string();
            if text.is_empty() {
                self.assistant_panel.drafts.remove(&previous);
            } else {
                self.assistant_panel.drafts.insert(previous, text);
            }
        }
        let draft = target
            .as_ref()
            .and_then(|target| self.assistant_panel.drafts.remove(target))
            .unwrap_or_default();
        self.assistant_panel
            .composer
            .update(cx, |composer, cx| composer.set_value(draft, window, cx));
        self.assistant_panel.composer_target = target;
        self.scroll_assistant_to_bottom(window, cx);
    }

    /// Asks Codex to load a conversation that the current process has not
    /// loaded. Codex sends a waiting tool call again when a loaded
    /// conversation resumes, so Qrow resumes each conversation once.
    fn load_assistant_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if matches!(self.assistant_panel.status, Status::Ready)
            && !self.assistant_panel.loaded_threads.contains(thread_id)
            && self.assistant_command(AssistantCommand::Resume(thread_id.to_owned()), cx)
        {
            self.assistant_panel
                .loaded_threads
                .insert(thread_id.to_owned());
        }
    }

    /// Detaches the conversation of a removed tab and drops the tab's draft.
    pub(super) fn assistant_tab_removed(&mut self, tab_id: Uuid, profile: Option<Uuid>) {
        let profile =
            profile.filter(|id| self.profiles.iter().any(|candidate| candidate.id == *id));
        // You closed the tab, so its unseen reply does not wait for you.
        if let Some(thread) = self
            .assistant
            .conversation_for_tab(tab_id)
            .map(|conversation| conversation.thread_id.clone())
        {
            self.thread_run_mut(&thread).unread = None;
        }
        self.assistant.detach_tab(tab_id, profile);
        self.assistant_panel
            .drafts
            .remove(&ComposerTarget::Tab(tab_id));
        self.assistant_panel.draft_modes.remove(&tab_id);
        self.assistant_panel.new_conversation_tabs.remove(&tab_id);
    }

    /// Ends the live state of all conversations after Codex stops. A first
    /// message returns to the draft of its tab.
    pub(super) fn reset_assistant_runs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for run in self.assistant_panel.runs.values_mut() {
            run.active_turn = None;
            run.pending_reply = false;
            run.target = None;
            run.pending_query = None;
            run.loading_older = false;
            run.sent_messages.clear();
        }
        self.assistant_panel.loaded_threads.clear();
        self.assistant_panel.pending_titles.clear();
        while let Some(first) = self.assistant_panel.first_messages.pop_front() {
            self.restore_first_message(first, window, cx);
        }
    }

    fn restore_first_message(
        &mut self,
        first: FirstMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restore_draft(ComposerTarget::Tab(first.tab_id), first.text, window, cx);
        self.assistant_panel
            .draft_modes
            .insert(first.tab_id, first.mode);
    }

    /// Puts the text of a message that Codex did not take before the draft
    /// of `target`.
    fn restore_draft(
        &mut self,
        target: ComposerTarget,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.assistant_panel.composer_target.as_ref() == Some(&target) {
            let current = self.assistant_panel.composer.read(cx).value().to_string();
            let text = if current.is_empty() {
                text
            } else {
                format!("{text}\n{current}")
            };
            self.assistant_panel
                .composer
                .update(cx, |composer, cx| composer.set_value(text, window, cx));
        } else {
            let text = match self.assistant_panel.drafts.remove(&target) {
                Some(draft) if !draft.is_empty() => format!("{text}\n{draft}"),
                _ => text,
            };
            self.assistant_panel.drafts.insert(target, text);
        }
    }

    /// Puts the oldest sent message of a conversation back in its message
    /// field after Codex rejects it.
    fn restore_sent_message(
        &mut self,
        thread_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.thread_run_mut(thread_id).sent_messages.pop_front() else {
            return;
        };
        if let Some(entries) = self.assistant_panel.transcripts.get_mut(thread_id)
            && let Some(position) = entries
                .iter()
                .rposition(|entry| entry.speaker == Speaker::User && entry.text == text)
        {
            entries.remove(position);
        }
        let target = match self
            .assistant
            .conversation(thread_id)
            .and_then(|conversation| conversation.tab_id)
        {
            Some(tab) => ComposerTarget::Tab(tab),
            None => ComposerTarget::Detached(thread_id.to_owned()),
        };
        self.restore_draft(target, text, window, cx);
    }

    fn load_older_assistant_messages(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.displayed_thread() else {
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
            self.assistant_panel.notice = Some(AssistantNotice::warning(
                "Codex repeated a conversation page. Older messages cannot be loaded.",
            ));
            cx.notify();
            return;
        }
        if self.thread_run(&thread_id).is_some_and(|run| {
            run.loading_older || run.active_turn.is_some() || run.pending_query.is_some()
        }) {
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
                .entry(thread_id.clone())
                .or_default()
                .insert(cursor);
            self.thread_run_mut(&thread_id).loading_older = true;
            cx.notify();
        }
    }

    /// Opens a new query tab for a new conversation. Codex creates the
    /// conversation when you send the first message.
    fn create_assistant_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        let index = self.add_tab(self.active_profile(), window, cx);
        self.assistant_panel
            .new_conversation_tabs
            .insert(self.tabs[index].saved.id);
        self.activate(index, window, cx);
        self.focus_assistant_composer(window, cx);
    }

    fn name_assistant_tab(&mut self, tab_id: Uuid, title: &str) {
        let Some(index) = self.tabs.iter().position(|tab| tab.saved.id == tab_id) else {
            return;
        };
        let profile = self.tabs[index].saved.profile;
        if let Some(title) = conversation_tab_title(title, |candidate| {
            self.tabs.iter().enumerate().any(|(other, tab)| {
                other != index && tab.saved.profile == profile && tab.saved.title == candidate
            })
        }) {
            self.tabs[index].saved.title = title;
        }
    }

    /// Selects a tab without a conversation and opens the pane for it. The
    /// first message starts the conversation in this tab.
    pub(super) fn start_tab_conversation(
        &mut self,
        tab_id: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() || self.tab_assistant_status(tab_id).is_some() {
            return;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.saved.id == tab_id) else {
            return;
        };
        self.activate(index, window, cx);
        if self.assistant_panel.open {
            self.focus_assistant_composer(window, cx);
        } else {
            self.toggle_assistant(window, cx);
        }
    }

    fn focus_assistant_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_panel.open {
            self.assistant_panel
                .composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
    }
    fn assistant_transcript_near_bottom(&self) -> bool {
        let scroll = &self.assistant_panel.scroll;
        scroll.offset().y + scroll.max_offset().y <= self.ui_px(32.)
    }

    fn scroll_assistant_to_bottom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.assistant_panel.scroll.scroll_to_bottom();
        cx.on_next_frame(window, |this, _, cx| {
            this.assistant_panel.scroll.scroll_to_bottom();
            cx.notify();
        });
    }

    fn begin_assistant_rename(
        &mut self,
        thread_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() {
            return;
        }
        let Some(conversation) = self
            .assistant
            .conversations
            .iter()
            .find(|conversation| conversation.thread_id == thread_id)
        else {
            return;
        };
        let title = conversation.title.clone();
        self.assistant_panel.rename_form = Some(ConversationEditor {
            thread_id: thread_id.to_owned(),
            title: cx.new(|cx| InputState::new(window, cx).default_value(title)),
            error: None,
        });
        self.open_rename_dialog(CONVERSATION_RENAME, window, cx);
        cx.notify();
    }

    fn save_assistant_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.assistant_panel.rename_form.as_mut() else {
            return;
        };
        let id = form.thread_id.clone();
        let title = form.title.read(cx).value().trim().to_owned();
        if title.is_empty() || title.chars().count() > MAX_ASSISTANT_CONVERSATION_TITLE {
            form.error = Some(format!(
                "Conversation name must have 1 to {MAX_ASSISTANT_CONVERSATION_TITLE} characters."
            ));
            cx.notify();
            return;
        }
        if !self.assistant_command(
            AssistantCommand::Rename {
                thread_id: id.clone(),
                title: title.clone(),
            },
            cx,
        ) {
            if let Some(form) = self.assistant_panel.rename_form.as_mut() {
                form.error = Some("Codex is not connected. Reconnect and try again.".into());
            }
            cx.notify();
            return;
        }
        // Codex cancels a title that is still generating.
        self.assistant_panel.regenerating_titles.remove(&id);
        self.assistant_panel.pending_titles.remove(&id);
        self.assistant_panel.title_history_reads.remove(&id);
        self.assistant_panel.pending_rename = Some((id, title));
        self.assistant_panel.rename_form = None;
        // Programmatic close_dialog does not invoke Dialog::on_close.
        window.close_dialog(cx);
        cx.notify();
    }

    /// Asks Codex for a new title, also when the user set the current title.
    fn regenerate_assistant_title(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if !self
            .assistant_panel
            .regenerating_titles
            .insert(thread_id.to_owned())
        {
            return;
        }
        let has_messages = self
            .assistant_panel
            .transcripts
            .get(thread_id)
            .is_some_and(|entries| entries.iter().any(|entry| entry.speaker == Speaker::User));
        let requested = if has_messages {
            self.send_assistant_title_request(thread_id, cx)
        } else {
            // Qrow loads a conversation's messages when you open it.
            self.assistant_panel
                .title_history_reads
                .insert(thread_id.to_owned());
            self.assistant_command(AssistantCommand::Read(thread_id.to_owned()), cx)
        };
        if !requested {
            self.assistant_panel.regenerating_titles.remove(thread_id);
            self.assistant_panel.title_history_reads.remove(thread_id);
        }
        cx.notify();
    }

    fn confirm_assistant_delete(
        &mut self,
        thread_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            let id = thread_id.clone();
            alert
                .title("Delete assistant conversation?")
                .description("Queries, logs, and results will not be lost.")
                .footer(
                    DialogFooter::new()
                        .justify_end()
                        .child(
                            Button::new("cancel-delete-assistant-conversation")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-delete-assistant-conversation")
                                .with_variant(ButtonVariant::Danger)
                                .label("Delete")
                                .on_click(move |_, window, cx| {
                                    let _ = confirm.update(cx, |this, cx| {
                                        this.assistant_command(
                                            AssistantCommand::Delete(id.clone()),
                                            cx,
                                        );
                                    });
                                    window.close_dialog(cx);
                                }),
                        ),
                )
        });
    }

    /// Builds the actions of the pane header menu and the thread list context
    /// menu.
    fn conversation_menu(&self, thread_id: &str, cx: &mut Context<Self>) -> ConversationMenu {
        let busy = self.thread_status(thread_id).busy();
        let id = thread_id.to_owned();
        ConversationMenu {
            busy,
            can_regenerate: !busy
                && !self.assistant_panel.unstarted_threads.contains(thread_id)
                && !self.assistant_panel.regenerating_titles.contains(thread_id),
            rename: Rc::new(cx.listener({
                let id = id.clone();
                move |this, _: &ClickEvent, window, cx| {
                    this.begin_assistant_rename(&id, window, cx);
                }
            })),
            regenerate: Rc::new(cx.listener({
                let id = id.clone();
                move |this, _: &ClickEvent, _, cx| this.regenerate_assistant_title(&id, cx)
            })),
            delete: Rc::new(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.confirm_assistant_delete(id.clone(), window, cx);
            })),
        }
    }

    /// Replaces saved assistant settings that the Codex snapshot does not offer.
    fn reconcile_assistant_settings(&mut self, cx: &mut Context<Self>) {
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
            self.assistant_panel.notice = Some(AssistantNotice::info(
                "Saved model is unavailable. Codex default model selected.",
            ));
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
        let efforts = model.map(|model| model.reasoning_efforts()).unwrap_or(&[]);
        if self
            .settings
            .assistant
            .reasoning_effort
            .as_ref()
            .is_some_and(|id| !efforts.iter().any(|effort| effort.id() == id))
        {
            self.settings.assistant.reasoning_effort = None;
            self.assistant_panel.notice = Some(AssistantNotice::info(
                "Saved reasoning level is unavailable. Codex default is in use.",
            ));
            self.changed(cx);
        }
        let tiers = model.map(|model| model.service_tiers()).unwrap_or(&[]);
        if self
            .settings
            .assistant
            .service_tier
            .as_ref()
            .is_some_and(|id| !tiers.iter().any(|tier| tier.id() == id))
        {
            self.settings.assistant.service_tier = None;
            self.assistant_panel.notice = Some(AssistantNotice::info(
                "Saved service tier is unavailable. Codex default is in use.",
            ));
            self.changed(cx);
        }
    }

    /// Shows a conversation. A closed conversation stays detached until its
    /// next message needs a query tab.
    fn select_assistant_thread(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog_open() {
            return;
        }
        if self.assistant.conversation(id).is_none() {
            return;
        }
        if let Some(index) = self.thread_tab_index(id) {
            self.activate(index, window, cx);
        } else {
            self.assistant_panel.browsed_thread = Some(id.to_owned());
            self.show_assistant_composer(Some(ComposerTarget::Detached(id.to_owned())), window, cx);
            if self.assistant_panel.open {
                self.thread_run_mut(id).unread = None;
            }
            self.load_assistant_thread(id, cx);
        }
        self.assistant_panel.thread_list_override =
            thread_list_after_selection(self.assistant_panel.thread_list_override);
        self.scroll_assistant_to_bottom(window, cx);
        self.focus_assistant_composer(window, cx);
        self.changed(cx);
    }

    fn select_assistant_model(&mut self, label: &str, cx: &mut Context<Self>) {
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
        self.changed(cx);
    }

    fn select_assistant_reasoning(&mut self, label: &str, cx: &mut Context<Self>) {
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
        self.changed(cx);
    }

    fn select_assistant_tier(&mut self, label: &str, cx: &mut Context<Self>) {
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
        self.changed(cx);
    }
    pub(super) fn toggle_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.settings.assistant.enabled {
            return;
        }
        self.assistant_panel.open = !self.assistant_panel.open;
        if self.assistant_panel.open {
            self.assistant_panel.previous_focus = window.focused(cx);
            if let Some(thread) = self.displayed_thread() {
                self.thread_run_mut(&thread).unread = None;
            }
            self.assistant_panel.idle_stop = None;
            self.start_assistant(cx);
            self.assistant_panel
                .composer
                .update(cx, |composer, cx| composer.focus(window, cx));
        } else {
            if let Some(focus) = self.assistant_panel.previous_focus.take() {
                focus.focus(window, cx);
            }
            if self.assistant_panel.service.is_some() {
                self.note_codex_activity(cx);
                self.schedule_idle_codex_stop(CODEX_IDLE_TIMEOUT, window, cx);
            }
        }
        cx.notify();
    }

    /// Starts the idle period again after a Codex command or event.
    fn note_codex_activity(&mut self, cx: &App) {
        self.assistant_panel.idle_since = cx.background_executor().now();
    }

    fn schedule_idle_codex_stop(
        &mut self,
        delay: Duration,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant_panel.idle_stop = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update_in(cx, |this, window, cx| this.stop_idle_codex(window, cx));
        }));
    }

    /// Stops Codex after `CODEX_IDLE_TIMEOUT` with the pane closed and without
    /// Codex activity. Work that a new process would lose delays the stop.
    fn stop_idle_codex(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = &self.assistant_panel;
        if panel.open || panel.service.is_none() {
            self.assistant_panel.idle_stop = None;
            return;
        }
        let idle = cx
            .background_executor()
            .now()
            .saturating_duration_since(panel.idle_since);
        if idle < CODEX_IDLE_TIMEOUT {
            self.schedule_idle_codex_stop(CODEX_IDLE_TIMEOUT - idle, window, cx);
        } else if self.codex_has_work() {
            // The end of the work is Codex activity, which starts the idle
            // period again.
            self.schedule_idle_codex_stop(CODEX_IDLE_TIMEOUT, window, cx);
        } else {
            self.assistant_panel.stop();
            self.reset_assistant_runs(window, cx);
            // The next open starts Codex like the first open, without an error.
            self.assistant_panel.status = Status::Idle;
            cx.notify();
        }
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
                self.assistant_panel.sign_in = SignIn::Idle;
                // A new Codex process has no title requests from the old one.
                self.assistant_panel.regenerating_titles.clear();
                self.assistant_panel.pending_titles.clear();
                self.assistant_panel.title_history_reads.clear();
            }
            Err(error) => {
                self.assistant_panel.status =
                    Status::Disconnected(format!("Could not start assistant: {error}"))
            }
        }
        cx.notify();
    }

    pub(super) fn reconnect_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.assistant_panel.stop();
        self.reset_assistant_runs(window, cx);
        self.start_assistant(cx);
    }

    pub(super) fn stop_assistant(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.displayed_thread() else {
            return;
        };
        let Some(turn_id) = self
            .thread_run(&thread_id)
            .and_then(|run| run.active_turn.clone())
        else {
            return;
        };
        self.assistant_command(
            AssistantCommand::Interrupt {
                thread_id: thread_id.clone(),
                turn_id,
            },
            cx,
        );
        self.thread_run_mut(&thread_id).pending_reply = false;
        cx.notify();
    }

    pub(super) fn toggle_assistant_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.mode_target() else {
            return;
        };
        if self.displayed_mode() == AssistantExecutionMode::RunAutomatically {
            self.set_mode(&target, AssistantExecutionMode::AskBeforeRunning, cx);
            return;
        }
        let weak = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let confirm = weak.clone();
            let target = target.clone();
            alert.title("Run assistant queries automatically?")
                .description("The assistant can run SQL that changes or deletes data and schema. Qrow cannot confirm that a statement is read-only.")
                .footer(DialogFooter::new().justify_end()
                    .child(Button::new("cancel-conversation-auto-run").label("Cancel")
                        .on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("confirm-conversation-auto-run").with_variant(ButtonVariant::Danger)
                        .label("Run automatically")
                        .on_click(move |_, window, cx| {
                            let _ = confirm.update(cx, |this, cx| {
                                this.set_mode(&target, AssistantExecutionMode::RunAutomatically, cx);
                            });
                            window.close_dialog(cx);
                        })))
        });
    }

    fn begin_assistant_sign_in(&mut self, cx: &mut Context<Self>) {
        if self.assistant_command(AssistantCommand::Login, cx) {
            self.assistant_panel.sign_in = SignIn::Starting;
            cx.notify();
        }
    }

    fn cancel_assistant_sign_in(&mut self, cx: &mut Context<Self>) {
        if let SignIn::Waiting { login_id, .. } = std::mem::take(&mut self.assistant_panel.sign_in)
        {
            self.assistant_command(AssistantCommand::CancelLogin(login_id), cx);
        }
        cx.notify();
    }

    /// Replaces the transcript while Codex has no account.
    fn assistant_sign_in(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let sign_in = &self.assistant_panel.sign_in;
        Empty::new()
            .size_full()
            .border_0()
            .header(
                EmptyHeader::new()
                    .media(
                        EmptyMedia::new()
                            .with_variant(EmptyMediaVariant::Icon)
                            .child(Icon::new(AssetIconName::Bot)),
                    )
                    .title(EmptyTitle::new().child("Sign in to Codex"))
                    .description(EmptyDescription::new().child(
                        "The assistant uses your Codex account. Sign in with ChatGPT to use your plan.",
                    )),
            )
            .content(match sign_in {
                SignIn::Waiting { url, .. } => {
                    let url = url.clone();
                    EmptyContent::new()
                        .child(
                            h_flex()
                                .id("assistant-sign-in-waiting")
                                .test_support()
                                .role(Role::Status)
                                .aria_label("Continue sign-in in your browser")
                                .gap_2()
                                .text_color(muted)
                                .child(Spinner::new().small().color(muted))
                                .child("Continue sign-in in your browser"),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("assistant-sign-in-reopen")
                                        .label("Reopen page")
                                        .tooltip("Reopen Sign-In Page")
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                                .child(
                                    Button::new("assistant-sign-in-cancel")
                                        .label("Cancel")
                                        .accessibility_label("Cancel Sign-In")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.cancel_assistant_sign_in(cx)
                                        })),
                                ),
                        )
                }
                SignIn::Idle | SignIn::Starting | SignIn::Failed(_) => EmptyContent::new()
                    .child(
                        Button::new("assistant-sign-in")
                            .label("Sign in with ChatGPT…")
                            .loading(*sign_in == SignIn::Starting)
                            .disabled(*sign_in == SignIn::Starting)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.begin_assistant_sign_in(cx)
                            })),
                    )
                    .when_some(
                        match sign_in {
                            SignIn::Failed(error) => Some(error.clone()),
                            _ => None,
                        },
                        |content, error| {
                            content.child(
                                v_flex()
                                    .id("assistant-sign-in-error")
                                    .test_support()
                                    .role(Role::Alert)
                                    .aria_label(format!("Couldn't sign in. {error}"))
                                    .gap_1()
                                    .child(
                                        div()
                                            .text_color(
                                                cx.theme().semantic_tokens().colors.destructive,
                                            )
                                            .child("Couldn't sign in. Try again."),
                                    )
                                    .child(div().text_xs().text_color(muted).child(error)),
                            )
                        },
                    ),
            })
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
            Some(Ok(())) => {
                self.note_codex_activity(cx);
                true
            }
            Some(Err(error)) => {
                self.assistant_panel.status = Status::Disconnected(error.into());
                cx.notify();
                false
            }
            None => false,
        }
    }

    /// Asks Codex for a title while the conversation still has the temporary title.
    fn request_assistant_title(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if self.assistant.conversations.iter().any(|conversation| {
            conversation.thread_id == thread_id
                && conversation.title_source == AssistantTitleSource::Temporary
        }) && !self.assistant_panel.pending_titles.contains(thread_id)
        {
            self.send_assistant_title_request(thread_id, cx);
        }
    }

    pub(super) fn assistant_title_generating(&self, thread_id: &str) -> bool {
        self.assistant_panel.pending_titles.contains(thread_id)
            || self.assistant_panel.regenerating_titles.contains(thread_id)
    }

    /// Sends the loaded messages of a conversation to Codex for a title.
    /// Returns false when the conversation has no user message or Codex is
    /// not connected.
    fn send_assistant_title_request(&mut self, thread_id: &str, cx: &mut Context<Self>) -> bool {
        let messages: Vec<_> = self
            .assistant_panel
            .transcripts
            .get(thread_id)
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry.speaker {
                Speaker::User => Some(("user", entry.text.clone())),
                Speaker::Assistant => Some(("assistant", entry.text.clone())),
                Speaker::Activity | Speaker::Error => None,
            })
            .collect();
        if !messages.iter().any(|(role, _)| *role == "user") {
            return false;
        }
        let model = self.assistant_panel.snapshot.as_ref().and_then(|snapshot| {
            self.settings
                .assistant
                .model
                .as_deref()
                .and_then(|id| snapshot.models().iter().find(|model| model.id() == id))
                .or_else(|| snapshot.models().iter().find(|model| model.is_default()))
        });
        // A title needs little reasoning. Models without this level use their default.
        let reasoning_effort = model
            .and_then(|model| {
                model
                    .reasoning_efforts()
                    .iter()
                    .find(|effort| effort.id() == "low")
            })
            .map(|effort| effort.id().to_owned());
        let requested = self.assistant_command(
            AssistantCommand::GenerateTitle(TitleRequest {
                thread_id: thread_id.to_owned(),
                messages,
                model: self.settings.assistant.model.clone(),
                reasoning_effort,
            }),
            cx,
        );
        if requested {
            self.assistant_panel
                .pending_titles
                .insert(thread_id.to_owned());
            cx.notify();
        }
        requested
    }

    pub(super) fn send_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.assistant_panel.status, Status::Ready) {
            return;
        }
        let text = self.assistant_panel.composer.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        if text.len() > MAX_MESSAGE_BYTES {
            // The message stays in the message field, so you can make it shorter.
            self.assistant_panel.notice = Some(AssistantNotice::warning(format!(
                "The message is too large. The limit is {} KB.",
                MAX_MESSAGE_BYTES / 1024
            )));
            cx.notify();
            return;
        }
        if let Some(thread_id) = self.displayed_thread() {
            let sent = if self.assistant_panel.browsed_thread.as_deref() == Some(thread_id.as_str())
            {
                self.send_detached_assistant_message(&thread_id, text, window, cx)
            } else {
                self.send_assistant_message(&thread_id, text, cx)
            };
            if !sent {
                return;
            }
        } else {
            let Some(tab_id) = self.active_tab_id() else {
                return;
            };
            if self
                .assistant_panel
                .first_messages
                .iter()
                .any(|first| first.tab_id == tab_id)
                || !self.assistant_command(AssistantCommand::Create(tools::definitions()), cx)
            {
                return;
            }
            let mode = self.displayed_mode();
            self.assistant_panel.draft_modes.remove(&tab_id);
            self.assistant_panel.first_messages.push_back(FirstMessage {
                tab_id,
                entry: TranscriptEntry::new(Speaker::User, text.clone(), None),
                text,
                mode,
            });
        }
        self.assistant_panel
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.scroll_assistant_to_bottom(window, cx);
        cx.notify();
    }

    /// Gives a closed conversation a new query tab only when its next message
    /// can start. A rejected message leaves the conversation detached.
    fn send_detached_assistant_message(
        &mut self,
        thread_id: &str,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(conversation) = self.assistant.conversation(thread_id) else {
            return false;
        };
        let previous_profile = conversation.detached_profile;
        let previous_follows = conversation.title_follows_conversation;
        let title = (conversation.title_source != AssistantTitleSource::Temporary)
            .then(|| conversation.title.clone());
        let profile = previous_profile
            .filter(|profile| {
                self.profiles
                    .iter()
                    .any(|candidate| candidate.id == *profile)
            })
            .or_else(|| self.active_profile());
        let index = self.add_tab(profile, window, cx);
        let tab = self.tabs[index].saved.id;
        if let Some(title) = title.as_deref() {
            self.name_assistant_tab(tab, title);
        }
        if let Some(conversation) = self.assistant.conversation_mut(thread_id) {
            conversation.tab_id = Some(tab);
            conversation.detached_profile = None;
            conversation.title_follows_conversation = true;
        }
        if !self.send_assistant_message(thread_id, text, cx) {
            if let Some(conversation) = self.assistant.conversation_mut(thread_id) {
                conversation.tab_id = None;
                conversation.detached_profile = previous_profile;
                conversation.title_follows_conversation = previous_follows;
            }
            self.tabs.remove(index);
            return false;
        }
        self.assistant_panel
            .composer
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.activate(index, window, cx);
        true
    }

    /// Starts a turn, or steers the active turn, of a conversation that has a
    /// query tab. Returns false when Codex did not take the message.
    fn send_assistant_message(
        &mut self,
        thread_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) -> bool {
        let context = self.assistant_context(thread_id, cx);
        let active_turn = self
            .thread_run(thread_id)
            .and_then(|run| run.active_turn.clone());
        let mut new_target = self.assistant_target(thread_id, cx);
        if let (Some(target), Some(turn_id)) = (&mut new_target, &active_turn) {
            target.turn_id = turn_id.clone();
        }
        let command = if let Some(turn_id) = active_turn.clone() {
            AssistantCommand::Steer {
                thread_id: thread_id.to_owned(),
                turn_id,
                text: text.clone(),
                context,
            }
        } else {
            AssistantCommand::Start(TurnRequest {
                thread_id: thread_id.to_owned(),
                text: text.clone(),
                context,
                model: self.settings.assistant.model.clone(),
                reasoning_effort: self.settings.assistant.reasoning_effort.clone(),
                service_tier: self.settings.assistant.service_tier.clone(),
            })
        };
        if !self.assistant_command(command, cx) {
            return false;
        }
        let run = self.thread_run_mut(thread_id);
        run.target = new_target;
        run.unread = None;
        run.pending_reply = true;
        run.sent_messages.push_back(text.clone());
        let replaced = run
            .pending_query
            .as_ref()
            .is_some_and(|pending| !pending.started)
            .then(|| run.pending_query.take())
            .flatten();
        if let Some(pending) = replaced {
            self.record_assistant_query_cancelled(
                &pending,
                "A new instruction replaced this query request.",
            );
            self.answer_assistant_call(pending.call, false, json!({"version":1,"error":{"code":"approval_cancelled","message":"A new instruction replaced this approval request."}}), cx);
        }
        self.assistant_panel.unstarted_threads.remove(thread_id);
        if let Some(conversation) = self.assistant.conversation_mut(thread_id) {
            conversation.last_activity = unix_now_seconds();
            self.changed(cx);
        }
        self.assistant_panel
            .transcripts
            .entry(thread_id.to_owned())
            .or_default()
            .push(TranscriptEntry::new(Speaker::User, text, active_turn));
        self.request_assistant_title(thread_id, cx);
        true
    }

    /// The action target of a conversation: its query tab, the connection of
    /// that tab, and the selection in it.
    pub(super) fn assistant_target(&self, thread_id: &str, cx: &App) -> Option<ActionTarget> {
        let tab = &self.tabs[self.thread_tab_index(thread_id)?];
        let selected = tab.input.read(cx).selected_range();
        Some(ActionTarget {
            conversation_id: thread_id.into(),
            turn_id: String::new(),
            tab_id: tab.saved.id,
            connection_id: tab.saved.profile,
            selected_range: (!selected.is_empty()).then_some(selected),
        })
    }

    /// The workspace context of a conversation. `selected_tab` is the
    /// conversation's query tab, also when you work in another tab.
    pub(super) fn assistant_context(&self, thread_id: &str, cx: &App) -> Value {
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
        let conversation_tab = self.thread_tab_index(thread_id);
        let selected_tab = conversation_tab.map(|index| {
            let tab = &self.tabs[index];
            let results = tab.table.read(cx);
            let results = results.delegate();
            let selection = tab.input.read(cx).selected_range();
            let error = tab
                .output
                .latest_error()
                .map(|entry| bound_text(&entry.text).0);
            let value = tab.input.read(cx).value();
            let sql: &str = &value;
            // A long tab sends only the part around the selection.
            let part = sql_window(sql, &selection, MAX_CONTEXT_SQL_BYTES);
            let (statement_ranges, statement_ranges_truncated) =
                context_statement_ranges(sql, &part);
            SelectedTabContext {
                tab: tabs[index].clone(),
                sql: sql[part.clone()].to_owned(),
                sql_offset: part.start,
                sql_bytes: sql.len(),
                sql_truncated: part.len() < sql.len(),
                selected_range: (!selection.is_empty()).then_some(selection),
                statement_ranges,
                statement_ranges_truncated,
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
        serde_json::to_value(WorkspaceContext::new(
            self.settings.sql_style(),
            connections,
            tabs,
            selected_tab,
        ))
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
        if changed {
            self.note_codex_activity(cx);
        }
        for event in events {
            self.handle_assistant_event(event, window, cx);
        }
        self.tick_assistant_queries(cx);
        if changed {
            cx.notify();
        }
        changed
    }

    fn update_assistant_snapshot(
        &mut self,
        snapshot: HarnessSnapshot,
        initial: bool,
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
        if matches!(self.assistant_panel.status, Status::Ready) {
            self.assistant_panel.sign_in = SignIn::Idle;
        }
        self.assistant_panel.snapshot = Some(snapshot);
        self.reconcile_assistant_settings(cx);
        if initial {
            // A new Codex process cannot resume conversations without a turn.
            let unstarted = std::mem::take(&mut self.assistant_panel.unstarted_threads);
            if !unstarted.is_empty() {
                self.assistant.remove_unstarted(&unstarted);
                self.assistant_panel
                    .transcripts
                    .retain(|thread, _| !unstarted.contains(thread));
                self.changed(cx);
            }
        }
        if initial {
            self.assistant_panel.loaded_threads.clear();
        }
        if let Some(thread) = self.displayed_thread() {
            self.load_assistant_thread(&thread, cx);
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
                self.update_assistant_snapshot(snapshot, true, cx)
            }
            AssistantServiceEvent::Snapshot(snapshot) => {
                self.update_assistant_snapshot(snapshot, false, cx)
            }
            AssistantServiceEvent::Created(conversation) => {
                // Codex creates conversations in the order of the requests.
                let Some(first) = self.assistant_panel.first_messages.pop_front() else {
                    return;
                };
                let id = conversation.id;
                self.assistant_panel.loaded_threads.insert(id.clone());
                self.assistant_panel.unstarted_threads.insert(id.clone());
                if !self.tabs.iter().any(|tab| tab.saved.id == first.tab_id)
                    || self.assistant.conversation_for_tab(first.tab_id).is_some()
                {
                    // Codex does not keep a conversation without a turn.
                    return;
                }
                let mut entry = AssistantConversation::new(id.clone(), first.mode);
                entry.last_activity = unix_now_seconds();
                entry.tab_id = Some(first.tab_id);
                entry.title_follows_conversation = self
                    .assistant_panel
                    .new_conversation_tabs
                    .remove(&first.tab_id);
                self.assistant.conversations.push(entry);
                if !self.send_assistant_message(&id, first.text.clone(), cx) {
                    self.restore_first_message(first, window, cx);
                }
                if self.displayed_thread().as_deref() == Some(id.as_str()) {
                    self.scroll_assistant_to_bottom(window, cx);
                }
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
                let selected = self.displayed_thread().as_deref() == Some(thread.as_str());
                let thread_id = thread.clone();
                let entries = self.assistant_panel.transcripts.entry(thread).or_default();
                let previous_count = entries.len();
                merge_history(entries, history.turns);
                if selected && entries.len() > previous_count {
                    self.scroll_assistant_to_bottom(window, cx);
                }
                if self.assistant_panel.title_history_reads.remove(&thread_id)
                    && !self.send_assistant_title_request(&thread_id, cx)
                {
                    self.assistant_panel.regenerating_titles.remove(&thread_id);
                    self.assistant_panel.notice = Some(AssistantNotice::info(
                        "This conversation has no messages for a title.",
                    ));
                }
            }
            AssistantServiceEvent::HistoryPage(page) => {
                self.thread_run_mut(&page.thread_id).loading_older = false;
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
            AssistantServiceEvent::Steered(thread_id) => {
                self.thread_run_mut(&thread_id).sent_messages.pop_front();
            }
            AssistantServiceEvent::TurnStarted { thread_id, turn } => {
                self.thread_run_mut(&thread_id).sent_messages.pop_front();
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
                let run = self.thread_run_mut(&thread_id);
                run.active_turn = Some(turn.id.clone());
                if let Some(target) = &mut run.target {
                    target.turn_id = turn.id;
                }
                if self.displayed_thread().as_deref() == Some(thread_id.as_str()) {
                    self.scroll_assistant_to_bottom(window, cx);
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::MessageDelta {
                thread_id,
                turn_id,
                text,
            }) => {
                let selected = self.displayed_thread().as_deref() == Some(thread_id.as_str());
                if !text.is_empty() {
                    self.thread_run_mut(&thread_id).pending_reply = false;
                }
                let entries = self
                    .assistant_panel
                    .transcripts
                    .entry(thread_id)
                    .or_default();
                let new_message = if let Some(last) = entries.last_mut().filter(|entry| {
                    entry.speaker == Speaker::Assistant
                        && entry.turn_id.as_deref() == Some(&turn_id)
                }) {
                    last.text.push_str(&text);
                    false
                } else {
                    entries.push(TranscriptEntry::new(
                        Speaker::Assistant,
                        text,
                        Some(turn_id),
                    ));
                    true
                };
                if selected && (new_message || self.assistant_transcript_near_bottom()) {
                    if new_message {
                        self.scroll_assistant_to_bottom(window, cx);
                    } else {
                        self.assistant_panel.scroll.scroll_to_bottom();
                    }
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::TurnCompleted {
                thread_id,
                turn,
                error,
            }) => {
                let viewed = self.assistant_panel.open
                    && self.displayed_thread().as_deref() == Some(thread_id.as_str());
                let run = self.thread_run_mut(&thread_id);
                run.pending_reply = false;
                let mut not_run = None;
                if run
                    .pending_query
                    .as_ref()
                    .is_some_and(|pending| pending.call.turn_id == turn.id)
                {
                    if run
                        .pending_query
                        .as_ref()
                        .is_some_and(|pending| pending.started)
                    {
                        if let Some(pending) = &mut run.pending_query {
                            pending.detached = true;
                        }
                    } else {
                        not_run = run.pending_query.take();
                    }
                }
                run.active_turn = None;
                run.target = None;
                if !viewed {
                    run.unread = Some(if error.is_some() {
                        Unread::Error
                    } else {
                        Unread::Reply
                    });
                }
                if let Some(pending) = not_run {
                    self.record_assistant_query_cancelled(
                        &pending,
                        "The turn ended before the query ran.",
                    );
                }
                self.assistant_command(AssistantCommand::Read(thread_id.clone()), cx);
                if let Some(position) = self
                    .assistant
                    .conversations
                    .iter()
                    .position(|conversation| conversation.thread_id == thread_id)
                {
                    let mut conversation = self.assistant.conversations.remove(position);
                    conversation.last_activity = unix_now_seconds();
                    self.assistant.conversations.insert(0, conversation);
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
                self.assistant_panel.pending_titles.remove(&thread_id);
                let regenerated = self.assistant_panel.regenerating_titles.remove(&thread_id);
                let mut tab_to_name = None;
                if let Some(conversation) = self
                    .assistant
                    .conversations
                    .iter_mut()
                    .find(|conversation| conversation.thread_id == thread_id)
                    && (regenerated || conversation.title_source != AssistantTitleSource::User)
                {
                    if conversation.title_follows_conversation {
                        tab_to_name = conversation.tab_id;
                    }
                    conversation.title = title.clone();
                    conversation.title_source = AssistantTitleSource::Codex;
                    if let Some(tab_id) = tab_to_name {
                        self.name_assistant_tab(tab_id, &title);
                    }
                    self.changed(cx);
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::TitleFailed { thread_id }) => {
                self.assistant_panel.pending_titles.remove(&thread_id);
                if self.assistant_panel.regenerating_titles.remove(&thread_id) {
                    self.assistant_panel.notice = Some(AssistantNotice::warning(
                        "Codex did not return a title. Try again.",
                    ));
                }
            }
            AssistantServiceEvent::Harness(AssistantEvent::ToolCall(call)) => {
                self.handle_assistant_tool(call, window, cx)
            }
            AssistantServiceEvent::Harness(AssistantEvent::Other { method, params }) => {
                if method == "account/login/completed" {
                    let success = params.get("success").and_then(Value::as_bool) == Some(true);
                    self.assistant_panel.sign_in.complete(
                        params.get("loginId").and_then(Value::as_str),
                        success,
                        params.get("error").and_then(Value::as_str),
                    );
                    if success {
                        self.assistant_command(AssistantCommand::Refresh, cx);
                    }
                } else if method == "account/updated" {
                    self.assistant_command(AssistantCommand::Refresh, cx);
                }
            }
            AssistantServiceEvent::LoginStarted(login) => {
                if self.assistant_panel.sign_in == SignIn::Starting {
                    cx.open_url(&login.url);
                    self.assistant_panel.sign_in = SignIn::Waiting {
                        login_id: login.login_id,
                        url: login.url,
                    };
                } else {
                    // The sign-in stopped before Codex answered.
                    self.assistant_command(AssistantCommand::CancelLogin(login.login_id), cx);
                }
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
                    let tab_to_name = conversation
                        .title_follows_conversation
                        .then_some(conversation.tab_id)
                        .flatten();
                    conversation.title = title.clone();
                    conversation.title_source = AssistantTitleSource::User;
                    if let Some(tab_id) = tab_to_name {
                        self.name_assistant_tab(tab_id, &title);
                    }
                    self.changed(cx);
                }
            }
            AssistantServiceEvent::Deleted(id) => {
                if self.assistant_panel.browsed_thread.as_deref() == Some(id.as_str()) {
                    self.assistant_panel.browsed_thread = None;
                    self.show_assistant_composer(
                        self.active_tab_id().map(ComposerTarget::Tab),
                        window,
                        cx,
                    );
                }
                self.assistant_panel
                    .drafts
                    .remove(&ComposerTarget::Detached(id.clone()));
                self.assistant_panel.runs.remove(&id);
                self.assistant_panel.loaded_threads.remove(&id);
                self.assistant_panel.older_cursors.remove(&id);
                self.assistant_panel.loaded_cursors.remove(&id);
                self.assistant
                    .conversations
                    .retain(|conversation| conversation.thread_id != id);
                self.assistant_panel.transcripts.remove(&id);
                self.assistant_panel.unstarted_threads.remove(&id);
                self.assistant_panel.regenerating_titles.remove(&id);
                self.assistant_panel.pending_titles.remove(&id);
                self.assistant_panel.title_history_reads.remove(&id);
                // The tab stays open without a conversation.
                self.changed(cx);
            }
            AssistantServiceEvent::Disconnected(error) => {
                self.assistant_panel.sign_in = SignIn::Idle;
                self.assistant_panel.status = Status::Disconnected(error);
                self.assistant_panel.service = None;
                self.assistant_panel.pending_titles.clear();
                self.reset_assistant_runs(window, cx);
            }
            AssistantServiceEvent::Failed {
                operation,
                id,
                error,
            } => {
                if operation == Operation::GenerateTitle {
                    if let Some(thread) = id.as_ref() {
                        self.assistant_panel.pending_titles.remove(thread);
                        if self.assistant_panel.regenerating_titles.remove(thread) {
                            self.assistant_panel.notice = Some(AssistantNotice::error(format!(
                                "Could not regenerate the title: {error}"
                            )));
                        }
                    }
                    // The temporary title stays until another message requests a title.
                    return;
                }
                if operation == Operation::Read
                    && let Some(thread) = id.as_ref()
                    && self.assistant_panel.title_history_reads.remove(thread)
                {
                    self.assistant_panel.regenerating_titles.remove(thread);
                    self.assistant_panel.notice = Some(AssistantNotice::error(format!(
                        "Could not regenerate the title: {error}"
                    )));
                    // The conversation shows its own error when you open it.
                    if self.displayed_thread().as_ref() != Some(thread) {
                        return;
                    }
                }
                if operation == Operation::Login {
                    self.assistant_panel.sign_in = SignIn::Failed(error);
                    return;
                }
                if operation == Operation::CancelLogin {
                    // Qrow no longer shows the sign-in. It ends when Codex stops.
                    return;
                }
                if operation == Operation::Rename {
                    self.assistant_panel.pending_rename = None;
                }
                if operation == Operation::Create {
                    if let Some(first) = self.assistant_panel.first_messages.pop_front() {
                        self.restore_first_message(first, window, cx);
                    }
                    self.assistant_panel.notice = Some(AssistantNotice::error(format!(
                        "Could not start the conversation: {error}"
                    )));
                    return;
                }
                if operation == Operation::Resume
                    && let Some(thread) = id.as_ref()
                {
                    // Qrow does not try again in this Codex process, so a tab
                    // change does not add the error again.
                    self.assistant_panel.loaded_threads.insert(thread.clone());
                }
                if operation == Operation::ReadOlder {
                    if let Some(thread) = id.as_ref() {
                        self.thread_run_mut(thread).loading_older = false;
                    }
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
                if let Some(thread) = id.as_ref()
                    && matches!(operation, Operation::Start | Operation::Steer)
                {
                    self.restore_sent_message(thread, window, cx);
                }
                if let Some(thread) = id.as_ref() {
                    let run = self.thread_run_mut(thread);
                    match operation {
                        Operation::Start => {
                            run.active_turn = None;
                            run.pending_reply = false;
                            run.target = None;
                            run.pending_query = None;
                        }
                        // The active turn continues without the message, and
                        // it can still wait for a tool answer.
                        Operation::Steer => run.pending_reply = false,
                        _ => {}
                    }
                }
                if operation == Operation::Answer {
                    self.assistant_panel.status = Status::Disconnected(error);
                    self.assistant_panel.service = None;
                    self.reset_assistant_runs(window, cx);
                    return;
                }
                if let Some(thread) = id.filter(|_| {
                    matches!(
                        operation,
                        Operation::Start
                            | Operation::Steer
                            | Operation::Interrupt
                            | Operation::Resume
                            | Operation::Read
                            | Operation::ReadOlder
                            | Operation::Delete
                            | Operation::Rename
                    )
                }) {
                    self.assistant_panel
                        .transcripts
                        .entry(thread)
                        .or_default()
                        .push(TranscriptEntry::new(Speaker::Error, error, None));
                } else {
                    // Codex still runs. The error does not belong to one
                    // conversation.
                    self.assistant_panel.notice = Some(AssistantNotice::error(error));
                }
            }
            _ => {}
        }
    }

    fn assistant_tool_entry(
        &self,
        thread: &str,
        index: usize,
        entry: &TranscriptEntry,
        tool: &ToolActivity,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let destructive = cx.theme().semantic_tokens().colors.destructive;
        let accessible_label = format!("Tool call: {}", entry.text);
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

    /// The icon of a conversation state. An idle conversation has no icon.
    pub(super) fn assistant_status_icon(
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

    /// Names the connection of a conversation. A conversation whose tab
    /// closed names the connection of its next tab.
    fn conversation_place(&self, conversation: &AssistantConversation) -> String {
        let (profile, closed) = match conversation
            .tab_id
            .and_then(|tab| self.tabs.iter().find(|candidate| candidate.saved.id == tab))
        {
            Some(tab) => (tab.saved.profile, false),
            None => (conversation.detached_profile, true),
        };
        let name = profile
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
            .map_or("No connection", |profile| profile.name.as_str());
        if closed {
            format!("{name} · Tab closed")
        } else {
            name.to_owned()
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
        let displayed = self.displayed_thread();
        v_flex()
            .w(if narrow { width } else { self.ui_px(230.) })
            .max_w_full()
            .h_full()
            .flex_shrink_0()
            .bg(cx.theme().sidebar)
            .when(!narrow, |list| list.border_l_1())
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
                                .accessibility_label("Back to Conversation")
                                .tooltip("Back to Conversation")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.assistant_panel.thread_list_override = Some(false);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(
                        Input::new(&self.assistant_panel.thread_search)
                            .small()
                            .flex_1()
                            .min_w_0()
                            .aria_label("Search Conversations"),
                    ),
            )
            .child(
                v_flex()
                    .id("assistant-thread-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_2()
                    .py_2()
                    .gap_1()
                    .children(
                        conversations
                            .into_iter()
                            .map(|conversation| {
                                (conversation, self.conversation_place(conversation))
                            })
                            .filter(|(conversation, place)| {
                                conversation.title.to_lowercase().contains(&search)
                                    || place.to_lowercase().contains(&search)
                            })
                            .map(|(conversation, place)| {
                                let id = conversation.thread_id.clone();
                                let status = self.thread_status(&id);
                                let row = h_flex()
                                    .id(SharedString::from(format!("assistant-thread-row-{id}")))
                                    .w_full()
                                    .h(self.ui_px(48.))
                                    .flex_shrink_0();
                                let selected = displayed.as_deref() == Some(id.as_str());
                                let label = conversation.title.clone();
                                let generating = self.assistant_title_generating(&id);
                                let accessible = format!(
                                    "{label}, {place}{}{}",
                                    status.accessible_suffix(),
                                    if generating { ", generating title" } else { "" }
                                );
                                let menu_thread = id.clone();
                                let button = Button::new(format!("assistant-thread-{id}"))
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h_full()
                                    .justify_start()
                                    .accessibility_label(accessible)
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_2()
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap_0p5()
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .when(generating, |title| {
                                                                title.child(
                                                                    ShimmerText::new(label.clone())
                                                                        .id(format!("assistant-title-row-{id}")),
                                                                )
                                                            })
                                                            .when(!generating, |title| title.child(label)),
                                                    )
                                                    .child(
                                                        div()
                                                            .min_w_0()
                                                            .truncate()
                                                            .text_xs()
                                                            .text_color(cx.theme().muted_foreground)
                                                            .child(format!(
                                                                "{place} · {}",
                                                                conversation_age(
                                                                    conversation.last_activity,
                                                                    unix_now_seconds(),
                                                                )
                                                            )),
                                                    ),
                                            )
                                            .when_some(
                                                self.assistant_status_icon(status, cx),
                                                |row, icon| {
                                                    row.child(div().flex_none().child(icon))
                                                },
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
                                    }));
                                row.child(button)
                                    .on_mouse_down(
                                        MouseButton::Right,
                                        cx.listener(move |_, event: &MouseDownEvent, window, cx| {
                                            let (thread, position) = (menu_thread.clone(), event.position);
                                            cx.defer_in(window, move |this, window, cx| {
                                                this.open_thread_menu(&thread, position, window, cx)
                                            });
                                        }),
                                    )
                                    .into_any_element()
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
                                    entry.text.clone(),
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
