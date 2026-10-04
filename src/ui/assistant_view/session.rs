//! The assistant state of a window: the Codex service, the live state of
//! each conversation, and the drafts of the message field.
use super::*;

#[derive(Clone, Debug)]
pub(in crate::ui) enum Status {
    Idle,
    Starting,
    SignInRequired,
    Ready,
    Disconnected(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::ui) enum AssistantNotice {
    Info(String),
    Warning(String),
    Error(String),
}

impl AssistantNotice {
    pub(super) fn info(message: impl Into<String>) -> Self {
        Self::Info(message.into())
    }

    pub(super) fn warning(message: impl Into<String>) -> Self {
        Self::Warning(message.into())
    }

    pub(super) fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }
}

/// Progress of a browser sign-in. Codex owns the account and the sign-in page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::ui) enum SignIn {
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
    pub(super) fn complete(&mut self, login_id: Option<&str>, success: bool, error: Option<&str>) {
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

pub(in crate::ui) struct PendingQuery {
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
pub(in crate::ui) enum PendingQueryKind {
    Run,
    Fetch,
}

/// A turn end that the user has not seen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum Unread {
    Reply,
    Error,
}

/// The live state of one conversation. Several conversations can have a turn
/// at the same time.
pub(in crate::ui) struct AppendedQuery {
    pub turn_id: String,
    pub tab_id: Uuid,
    pub connection_id: Option<Uuid>,
    pub revision: u64,
    pub selected_range: Range<usize>,
}

#[derive(Default)]
pub(in crate::ui) struct ThreadRun {
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
    /// Catalog tool calls that wait for the cache or for a refresh.
    pub catalog_calls: Vec<super::super::assistant_tools::PendingCatalogCall>,
}

/// The conversation state that the thread list and the assistant toggle show.
/// A higher state is more urgent.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::ui) enum ThreadStatus {
    Idle,
    Ready,
    Working,
    Failed,
    Approval,
}

impl ThreadStatus {
    pub(in crate::ui) fn tooltip_status(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Ready => "Unread Reply",
            Self::Working => "Working",
            Self::Failed => "Unread Error",
            Self::Approval => "Needs Approval",
        }
    }

    pub(in crate::ui) fn dot_status(self) -> Option<DotStatus> {
        match self {
            Self::Idle => None,
            Self::Working => Some(DotStatus::Working),
            Self::Ready => Some(DotStatus::Ready),
            Self::Failed => Some(DotStatus::Error),
            Self::Approval => Some(DotStatus::Attention),
        }
    }

    pub(super) fn of(run: &ThreadRun) -> Self {
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
pub(in crate::ui) struct FirstMessage {
    pub tab_id: Uuid,
    pub text: String,
    pub mode: AssistantExecutionMode,
    /// Shows the message until the conversation has it.
    pub entry: TranscriptEntry,
}

/// A conversation, or a tab that does not have one yet.
#[derive(Clone, Debug)]
pub(super) enum ModeTarget {
    Thread(String),
    Tab(Uuid),
}

/// The time that quit waits for Codex to stop, including a forced stop.
pub(super) const QUIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Qrow stops Codex when the pane stays closed for this time without Codex
/// work. The next open starts Codex again.
pub const CODEX_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(in crate::ui) enum ComposerTarget {
    Tab(Uuid),
    Detached(String),
}

/// The assistant state of a window that outlives the pane: Codex, the
/// conversations, and their drafts. Qrow owns it, because tabs, quit, and
/// the workspace file use it too. The pane view keeps its own view state.
pub(in crate::ui) struct AssistantState {
    pub open: bool,
    pub status: Status,
    pub service: Option<Service>,
    pub snapshot: Option<HarnessSnapshot>,
    /// A closed conversation shown without opening a query tab.
    pub browsed_thread: Option<String>,
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
    pub idle_stop: Option<Task<()>>,
    /// The last Codex command or event, or the time that the pane closed.
    pub idle_since: Instant,
}

impl AssistantState {
    pub fn new(cx: &App) -> Self {
        Self {
            open: false,
            status: Status::Idle,
            service: None,
            snapshot: None,
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

/// What the pane shows: the displayed conversation, or the first message of
/// the active tab before Codex creates its conversation.
pub(in crate::ui) struct Shown<'a> {
    pub thread: Option<String>,
    pub run: Option<&'a ThreadRun>,
    pub first_message: Option<&'a FirstMessage>,
    pub pending_approval: Option<&'a PendingQuery>,
    /// Codex owes a reply from the send until the turn ends. A query that
    /// waits for approval waits for you instead.
    pub waiting_for_agent: bool,
    pub entries: &'a [TranscriptEntry],
}

impl Shown<'_> {
    /// The rows of the transcript list: the button for older messages, the
    /// entries, and the line that shows while Codex works.
    pub fn rows(&self, has_older: bool, active_tab: Option<Uuid>) -> TranscriptRows {
        let entries = self
            .entries
            .iter()
            .chain(self.first_message.map(|first| &first.entry))
            .map(|entry| Row::Entry {
                id: entry.id,
                revision: entry.revision(),
            });
        TranscriptRows {
            source: match (&self.thread, active_tab) {
                (Some(thread), _) => Some(thread.clone()),
                (None, Some(tab)) => Some(tab.to_string()),
                (None, None) => None,
            },
            rows: has_older
                .then_some(Row::Older)
                .into_iter()
                .chain(entries)
                .chain(self.waiting_for_agent.then_some(Row::Working))
                .collect(),
        }
    }

    /// The entry `id` of the transcript or of the first message.
    pub fn entry(&self, id: Uuid, index: usize) -> Option<&TranscriptEntry> {
        let first = self.first_message.map(|first| &first.entry);
        self.entries
            .get(index)
            .filter(|entry| entry.id == id)
            .or_else(|| {
                self.entries
                    .iter()
                    .chain(first)
                    .find(|entry| entry.id == id)
            })
    }
}

impl Qrow {
    pub(in crate::ui) fn active_tab_id(&self) -> Option<Uuid> {
        self.tabs.get(self.active).map(|tab| tab.saved.id)
    }

    /// The conversation shown in the pane, including a selected closed thread.
    pub(in crate::ui) fn displayed_thread(&self) -> Option<String> {
        if let Some(thread) = self.assistant_state.browsed_thread.as_ref()
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

    pub(in crate::ui) fn shown_conversation(&self) -> Shown<'_> {
        let thread = self.displayed_thread();
        let run = thread.as_deref().and_then(|thread| self.thread_run(thread));
        let active_tab = self.active_tab_id();
        let first_message = self.assistant_state.first_messages.iter().find(|first| {
            self.assistant_state.browsed_thread.is_none() && Some(first.tab_id) == active_tab
        });
        let pending_approval = run
            .and_then(|run| run.pending_query.as_ref())
            .filter(|pending| !pending.approved);
        let waiting_for_agent = first_message.is_some()
            || (run.is_some_and(|run| run.pending_reply || run.active_turn.is_some())
                && pending_approval.is_none());
        let entries = thread
            .as_deref()
            .and_then(|thread| self.assistant_state.transcripts.get(thread))
            .map_or(&[][..], Vec::as_slice);
        Shown {
            thread,
            run,
            first_message,
            pending_approval,
            waiting_for_agent,
            entries,
        }
    }

    /// The rows of the transcript that the pane shows.
    pub(in crate::ui) fn assistant_rows(&self) -> TranscriptRows {
        let shown = self.shown_conversation();
        let has_older = shown
            .thread
            .as_ref()
            .is_some_and(|thread| self.assistant_state.older_cursors.contains_key(thread));
        shown.rows(has_older, self.active_tab_id())
    }

    /// The index of a conversation's query tab.
    pub(in crate::ui) fn thread_tab_index(&self, thread_id: &str) -> Option<usize> {
        let tab = self.assistant.conversation(thread_id)?.tab_id?;
        self.tabs
            .iter()
            .position(|candidate| candidate.saved.id == tab)
    }

    pub(in crate::ui) fn thread_run(&self, thread_id: &str) -> Option<&ThreadRun> {
        self.assistant_state.runs.get(thread_id)
    }

    pub(in crate::ui) fn thread_run_mut(&mut self, thread_id: &str) -> &mut ThreadRun {
        self.assistant_state
            .runs
            .entry(thread_id.to_owned())
            .or_default()
    }

    pub(in crate::ui) fn thread_status(&self, thread_id: &str) -> ThreadStatus {
        self.thread_run(thread_id)
            .map_or(ThreadStatus::Idle, ThreadStatus::of)
    }

    /// The assistant state of a query tab, or `None` for a tab without a
    /// conversation.
    pub(in crate::ui) fn tab_assistant_status(&self, tab_id: Uuid) -> Option<ThreadStatus> {
        if self
            .assistant_state
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
    pub(in crate::ui) fn assistant_tab_busy(&self, tab_id: Uuid) -> bool {
        self.tab_assistant_status(tab_id)
            .is_some_and(ThreadStatus::busy)
    }

    /// Whether a conversation has a turn that quitting would stop.
    pub(in crate::ui) fn assistant_working(&self) -> bool {
        !self.assistant_state.first_messages.is_empty()
            || self
                .assistant_state
                .runs
                .values()
                .any(|run| run.active_turn.is_some() || run.pending_reply)
    }

    /// Whether Codex has work that a new Codex process would lose. A new
    /// process cannot resume a conversation without a turn, and it does not
    /// know the requests, tool calls, and sign-in of the old process.
    pub(super) fn codex_has_work(&self) -> bool {
        let panel = &self.assistant_state;
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
    pub(in crate::ui) fn assistant_status(&self) -> ThreadStatus {
        let first = if self.assistant_state.first_messages.is_empty() {
            ThreadStatus::Idle
        } else {
            ThreadStatus::Working
        };
        self.assistant_state
            .runs
            .values()
            .map(ThreadStatus::of)
            .fold(first, Ord::max)
    }

    pub(super) fn mode_target(&self) -> Option<ModeTarget> {
        self.displayed_thread()
            .map(ModeTarget::Thread)
            .or_else(|| self.active_tab_id().map(ModeTarget::Tab))
    }

    /// The query mode of the displayed conversation. A tab without a
    /// conversation uses the mode for its first message.
    pub(in crate::ui) fn displayed_mode(&self) -> AssistantExecutionMode {
        match self.mode_target() {
            Some(ModeTarget::Thread(thread)) => self
                .assistant
                .conversation(&thread)
                .map(|conversation| conversation.execution_mode)
                .unwrap_or_default(),
            Some(ModeTarget::Tab(tab)) => self
                .assistant_state
                .draft_modes
                .get(&tab)
                .copied()
                .unwrap_or(self.settings.assistant.default_execution_mode),
            None => self.settings.assistant.default_execution_mode,
        }
    }

    pub(super) fn set_mode(
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
                self.assistant_state.draft_modes.insert(*tab, mode);
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(ThreadStatus::Failed > ThreadStatus::Working);
        assert!(ThreadStatus::Working > ThreadStatus::Ready);
    }
}
