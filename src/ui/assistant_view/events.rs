//! Codex events: replies, history, titles, tool calls, and failures.
use super::*;

impl Qrow {
    pub(in crate::ui) fn tick_assistant(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
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

    pub(super) fn update_assistant_snapshot(
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

    pub(super) fn handle_assistant_event(
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
                    last.push_text(&text);
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
}
