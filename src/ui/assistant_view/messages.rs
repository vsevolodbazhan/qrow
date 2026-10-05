//! Sending messages, and the workspace context that each message includes.
use super::*;

impl Qrow {
    pub(in crate::ui) fn send_assistant(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.assistant_state.status, Status::Ready) {
            return;
        }
        let text = self.assistant_composer(cx).read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        if text.len() > MAX_MESSAGE_BYTES {
            // The message stays in the message field, so you can make it shorter.
            self.assistant_state.notice = Some(AssistantNotice::warning(format!(
                "The message is too large. The limit is {} KB.",
                MAX_MESSAGE_BYTES / 1024
            )));
            cx.notify();
            return;
        }
        if let Some(thread_id) = self.displayed_thread() {
            let sent = if self.assistant_state.browsed_thread.as_deref() == Some(thread_id.as_str())
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
                .assistant_state
                .first_messages
                .iter()
                .any(|first| first.tab_id == tab_id)
                || !self.assistant_command(AssistantCommand::Create(tools::definitions()), cx)
            {
                return;
            }
            let mode = self.displayed_mode();
            self.assistant_state.draft_modes.remove(&tab_id);
            self.assistant_state.first_messages.push_back(FirstMessage {
                tab_id,
                entry: TranscriptEntry::new(Speaker::User, text.clone(), None),
                text,
                mode,
            });
        }
        self.assistant_composer(cx)
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.scroll_assistant_to_bottom(cx);
        cx.notify();
    }

    /// Gives a closed conversation a new query tab only when its next message
    /// can start. A rejected message leaves the conversation detached.
    pub(super) fn send_detached_assistant_message(
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
        self.assistant_composer(cx)
            .update(cx, |composer, cx| composer.set_value("", window, cx));
        self.activate(index, window, cx);
        true
    }

    /// Starts a turn, or steers the active turn, of a conversation that has a
    /// query tab. Returns false when Codex did not take the message.
    pub(super) fn send_assistant_message(
        &mut self,
        thread_id: &str,
        text: String,
        cx: &mut Context<Self>,
    ) -> bool {
        // The catalog tools read the cache of the tab connection. Load it now,
        // so it is ready when the assistant asks.
        if let Some(profile) = self
            .thread_tab_index(thread_id)
            .and_then(|index| self.tabs[index].saved.profile)
        {
            self.ensure_catalog(profile);
        }
        let (notes, sent_notes) = self.assistant_notes(thread_id);
        let context = self.assistant_context(thread_id, notes, cx);
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
        run.pending_notes.push_back(sent_notes);
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
        self.assistant_state.unstarted_threads.remove(thread_id);
        if let Some(conversation) = self.assistant.conversation_mut(thread_id) {
            conversation.last_activity = unix_now_seconds();
            // Codex has the notes only when it takes the message.
            conversation.sent_notes = Some(crate::assistant::notes::unknown());
            self.changed(cx);
        }
        self.assistant_state
            .transcripts
            .entry(thread_id.to_owned())
            .or_default()
            .push(TranscriptEntry::new(Speaker::User, text, active_turn));
        self.request_assistant_title(thread_id, cx);
        true
    }

    /// The action target of a conversation: its query tab, the connection of
    /// that tab, and the selection in it.
    pub(in crate::ui) fn assistant_target(
        &self,
        thread_id: &str,
        cx: &App,
    ) -> Option<ActionTarget> {
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

    /// The notes of the connection of a conversation for its next message,
    /// and the record to keep when Codex takes the message.
    fn assistant_notes(
        &self,
        thread_id: &str,
    ) -> (
        crate::assistant::notes::ContextNotes,
        crate::model::SentNotes,
    ) {
        let (connection, notes) = self.conversation_connection_notes(thread_id);
        let sent = self
            .assistant
            .conversation(thread_id)
            .and_then(|conversation| conversation.sent_notes.as_ref());
        crate::assistant::notes::for_message(sent, connection, notes)
    }

    /// Keeps the notes record of the oldest pending message of a
    /// conversation when Codex takes the message. The record stays unknown
    /// while later messages are pending, or after a workspace read during
    /// the wait.
    pub(super) fn acknowledge_notes(
        &mut self,
        thread_id: &str,
        taken: bool,
        cx: &mut Context<Self>,
    ) {
        let run = self.thread_run_mut(thread_id);
        let Some(record) = run.pending_notes.pop_front() else {
            return;
        };
        if !run.pending_notes.is_empty() {
            return;
        }
        let known = taken && !run.notes_read_while_pending;
        run.notes_read_while_pending = false;
        if known && let Some(conversation) = self.assistant.conversation_mut(thread_id) {
            conversation.sent_notes = Some(record);
            self.changed(cx);
        }
    }

    /// Keeps the notes that a workspace read gave to Codex.
    pub(in crate::ui) fn notes_read(&mut self, thread_id: &str, record: crate::model::SentNotes) {
        let run = self.thread_run_mut(thread_id);
        if !run.pending_notes.is_empty() {
            run.notes_read_while_pending = true;
        } else if let Some(conversation) = self.assistant.conversation_mut(thread_id) {
            conversation.sent_notes = Some(record);
        }
    }

    /// The connection of the query tab of a conversation, and its notes.
    pub(in crate::ui) fn conversation_connection_notes(
        &self,
        thread_id: &str,
    ) -> (Option<Uuid>, &str) {
        let connection = self
            .thread_tab_index(thread_id)
            .and_then(|index| self.tabs[index].saved.profile);
        let notes = connection
            .and_then(|id| self.profiles.iter().find(|profile| profile.id == id))
            .map_or("", |profile| profile.assistant_notes.as_str());
        (connection, notes)
    }

    /// The workspace context of a conversation. `selected_tab` is the
    /// conversation's query tab, also when you work in another tab.
    pub(in crate::ui) fn assistant_context(
        &self,
        thread_id: &str,
        notes: crate::assistant::notes::ContextNotes,
        cx: &App,
    ) -> Value {
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
        let catalog = conversation_tab.and_then(|index| {
            let tab = &self.tabs[index];
            let profile = self
                .profiles
                .iter()
                .find(|profile| Some(profile.id) == tab.saved.profile)?;
            let shared = profile.shared_catalog.and_then(|id| {
                self.shared_catalogs
                    .iter()
                    .find(|catalog| catalog.id == id)
                    .map(|catalog| catalog.name.clone())
            });
            Some(crate::assistant::catalog::CatalogContext::new(
                profile.id,
                profile.catalog.browses(),
                shared,
                self.catalog.catalog(profile.id),
                &crate::model::effective_catalog(profile, &self.shared_catalogs),
                &tab.input.read(cx).value(),
                &profile.database,
                crate::catalog::now(),
            ))
        });
        serde_json::to_value(
            WorkspaceContext::new(self.settings.sql_style(), connections, tabs, selected_tab)
                .with_catalog(catalog)
                .with_notes(notes),
        )
        .unwrap_or(json!({"version": 1}))
    }
}
