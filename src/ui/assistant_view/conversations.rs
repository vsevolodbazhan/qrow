//! Showing, creating, renaming, and deleting conversations, and their titles.
use super::*;

/// The open rename dialog for one conversation.
pub(in crate::ui) struct ConversationEditor {
    pub(super) thread_id: String,
    pub(super) title: Entity<InputState>,
    pub(super) error: Option<String>,
}

pub(super) const CONVERSATION_RENAME: tab_view::RenameDialog = tab_view::RenameDialog {
    key: "conversation",
    label: "Conversation Name",
    tooltip: "Rename Conversation · ⌘Enter",
    form: |this| {
        this.assistant_state
            .rename_form
            .as_ref()
            .map(|form| (&form.title, form.error.clone()))
    },
    submit: Qrow::save_assistant_rename,
    clear: |this| this.assistant_state.rename_form = None,
};

pub(super) type MenuAction = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The actions for one conversation. The pane header menu and the thread list
/// context menu use the same items and layout.
pub(super) struct ConversationMenu {
    pub(super) busy: bool,
    pub(super) can_regenerate: bool,
    pub(super) rename: MenuAction,
    pub(super) regenerate: MenuAction,
    pub(super) delete: MenuAction,
}

impl Qrow {
    /// The menu of a thread list row. Qrow owns it like the tab menu; GPUI
    /// Kit's `context_menu` keeps each dismissed menu alive.
    pub(super) fn open_thread_menu(
        &mut self,
        thread: &str,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.conversation_menu(thread, &cx.weak_entity());
        self.open_context_menu(position, move |popup, _, _| menu.build(popup), window, cx);
    }
}

impl ConversationMenu {
    pub(super) fn build(&self, menu: PopupMenu) -> PopupMenu {
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

impl Qrow {
    /// Shows the conversation of the active tab after a tab change.
    pub(in crate::ui) fn show_tab_conversation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.assistant_state.browsed_thread = None;
        let target = self.active_tab_id().map(ComposerTarget::Tab);
        self.show_assistant_composer(target, window, cx);
        if let Some(thread) = self.displayed_thread() {
            if self.assistant_state.open {
                self.thread_run_mut(&thread).unread = None;
            }
            self.load_assistant_thread(&thread, cx);
        }
    }

    /// Keeps an unsent message with the tab or closed conversation that owns it.
    pub(super) fn show_assistant_composer(
        &mut self,
        target: Option<ComposerTarget>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.assistant_state.composer_target == target {
            return;
        }
        if let Some(previous) = self.assistant_state.composer_target.take()
            && match &previous {
                ComposerTarget::Tab(tab) => self.tabs.iter().any(|item| item.saved.id == *tab),
                ComposerTarget::Detached(thread) => self.assistant.conversation(thread).is_some(),
            }
        {
            let text = self.assistant_composer(cx).read(cx).value().to_string();
            if text.is_empty() {
                self.assistant_state.drafts.remove(&previous);
            } else {
                self.assistant_state.drafts.insert(previous, text);
            }
        }
        let draft = target
            .as_ref()
            .and_then(|target| self.assistant_state.drafts.remove(target))
            .unwrap_or_default();
        self.assistant_composer(cx)
            .update(cx, |composer, cx| composer.set_value(draft, window, cx));
        self.assistant_state.composer_target = target;
        self.scroll_assistant_to_bottom(cx);
    }

    /// Asks Codex to load a conversation that the current process has not
    /// loaded. Codex sends a waiting tool call again when a loaded
    /// conversation resumes, so Qrow resumes each conversation once.
    pub(super) fn load_assistant_thread(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if matches!(self.assistant_state.status, Status::Ready)
            && !self.assistant_state.loaded_threads.contains(thread_id)
            && self.assistant_command(AssistantCommand::Resume(thread_id.to_owned()), cx)
        {
            self.assistant_state
                .loaded_threads
                .insert(thread_id.to_owned());
        }
    }

    /// Detaches the conversation of a removed tab and drops the tab's draft.
    pub(in crate::ui) fn assistant_tab_removed(&mut self, tab_id: Uuid, profile: Option<Uuid>) {
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
        self.assistant_state
            .drafts
            .remove(&ComposerTarget::Tab(tab_id));
        self.assistant_state.draft_modes.remove(&tab_id);
        self.assistant_state.new_conversation_tabs.remove(&tab_id);
    }

    /// Ends the live state of all conversations after Codex stops. A first
    /// message returns to the draft of its tab.
    pub(in crate::ui) fn reset_assistant_runs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for run in self.assistant_state.runs.values_mut() {
            run.active_turn = None;
            run.pending_reply = false;
            run.target = None;
            run.pending_query = None;
            run.loading_older = false;
            run.sent_messages.clear();
        }
        self.assistant_state.loaded_threads.clear();
        self.assistant_state.pending_titles.clear();
        while let Some(first) = self.assistant_state.first_messages.pop_front() {
            self.restore_first_message(first, window, cx);
        }
    }

    pub(super) fn restore_first_message(
        &mut self,
        first: FirstMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.restore_draft(ComposerTarget::Tab(first.tab_id), first.text, window, cx);
        self.assistant_state
            .draft_modes
            .insert(first.tab_id, first.mode);
    }

    /// Puts the text of a message that Codex did not take before the draft
    /// of `target`.
    pub(super) fn restore_draft(
        &mut self,
        target: ComposerTarget,
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.assistant_state.composer_target.as_ref() == Some(&target) {
            let current = self.assistant_composer(cx).read(cx).value().to_string();
            let text = if current.is_empty() {
                text
            } else {
                format!("{text}\n{current}")
            };
            self.assistant_composer(cx)
                .update(cx, |composer, cx| composer.set_value(text, window, cx));
        } else {
            let text = match self.assistant_state.drafts.remove(&target) {
                Some(draft) if !draft.is_empty() => format!("{text}\n{draft}"),
                _ => text,
            };
            self.assistant_state.drafts.insert(target, text);
        }
    }

    /// Puts the oldest sent message of a conversation back in its message
    /// field after Codex rejects it.
    pub(super) fn restore_sent_message(
        &mut self,
        thread_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(text) = self.thread_run_mut(thread_id).sent_messages.pop_front() else {
            return;
        };
        if let Some(entries) = self.assistant_state.transcripts.get_mut(thread_id)
            && let Some(position) = entries
                .iter()
                .rposition(|entry| entry.speaker == Speaker::User && *entry.text() == text)
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

    pub(super) fn load_older_assistant_messages(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.displayed_thread() else {
            return;
        };
        let Some(cursor) = self.assistant_state.older_cursors.get(&thread_id).cloned() else {
            return;
        };
        if self
            .assistant_state
            .loaded_cursors
            .entry(thread_id.clone())
            .or_default()
            .contains(&cursor)
        {
            self.assistant_state.older_cursors.remove(&thread_id);
            self.assistant_state.notice = Some(AssistantNotice::warning(
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
            self.assistant_state
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
    pub(super) fn create_assistant_conversation(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() {
            return;
        }
        let index = self.add_tab(self.active_profile(), window, cx);
        self.assistant_state
            .new_conversation_tabs
            .insert(self.tabs[index].saved.id);
        self.activate(index, window, cx);
        self.focus_assistant_composer(window, cx);
    }

    pub(super) fn name_assistant_tab(&mut self, tab_id: Uuid, title: &str) {
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
    pub(in crate::ui) fn start_tab_conversation(
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
        if self.assistant_state.open {
            self.focus_assistant_composer(window, cx);
        } else {
            self.toggle_assistant(window, cx);
        }
    }

    pub(super) fn focus_assistant_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.assistant_state.open {
            self.assistant_composer(cx)
                .update(cx, |composer, cx| composer.focus(window, cx));
        }
    }

    /// The message field of the pane.
    pub(in crate::ui) fn assistant_composer(&self, cx: &App) -> Entity<TextareaState> {
        self.assistant_pane.read(cx).composer().clone()
    }

    /// Shows the end of the transcript, which then follows new text.
    pub(super) fn scroll_assistant_to_bottom(&self, cx: &mut App) {
        self.assistant_pane
            .update(cx, |pane, cx| pane.scroll_to_end(cx));
    }

    /// Gives the pane the rows of the shown transcript after a change that
    /// Qrow does not render.
    pub(super) fn sync_assistant_pane(&self, cx: &mut App) {
        let rows = self.assistant_rows();
        self.assistant_pane
            .update(cx, |pane, cx| pane.sync_rows(rows, cx));
    }

    pub(super) fn begin_assistant_rename(
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
        self.assistant_state.rename_form = Some(ConversationEditor {
            thread_id: thread_id.to_owned(),
            title: cx.new(|cx| InputState::new(window, cx).default_value(title)),
            error: None,
        });
        self.open_rename_dialog(CONVERSATION_RENAME, window, cx);
        cx.notify();
    }

    pub(super) fn save_assistant_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(form) = self.assistant_state.rename_form.as_mut() else {
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
            if let Some(form) = self.assistant_state.rename_form.as_mut() {
                form.error = Some("Codex is not connected. Reconnect and try again.".into());
            }
            cx.notify();
            return;
        }
        // Codex cancels a title that is still generating.
        self.assistant_state.regenerating_titles.remove(&id);
        self.assistant_state.pending_titles.remove(&id);
        self.assistant_state.title_history_reads.remove(&id);
        self.assistant_state.pending_rename = Some((id, title));
        self.assistant_state.rename_form = None;
        // Programmatic close_dialog does not invoke Dialog::on_close.
        window.close_dialog(cx);
        cx.notify();
    }

    /// Asks Codex for a new title, also when the user set the current title.
    pub(super) fn regenerate_assistant_title(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if !self
            .assistant_state
            .regenerating_titles
            .insert(thread_id.to_owned())
        {
            return;
        }
        let has_messages = self
            .assistant_state
            .transcripts
            .get(thread_id)
            .is_some_and(|entries| entries.iter().any(|entry| entry.speaker == Speaker::User));
        let requested = if has_messages {
            self.send_assistant_title_request(thread_id, cx)
        } else {
            // Qrow loads a conversation's messages when you open it.
            self.assistant_state
                .title_history_reads
                .insert(thread_id.to_owned());
            self.assistant_command(AssistantCommand::Read(thread_id.to_owned()), cx)
        };
        if !requested {
            self.assistant_state.regenerating_titles.remove(thread_id);
            self.assistant_state.title_history_reads.remove(thread_id);
        }
        cx.notify();
    }

    pub(super) fn confirm_assistant_delete(
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
    pub(super) fn conversation_menu(
        &self,
        thread_id: &str,
        handle: &WeakEntity<Qrow>,
    ) -> ConversationMenu {
        let busy = self.thread_status(thread_id).busy();
        let id = thread_id.to_owned();
        ConversationMenu {
            busy,
            can_regenerate: !busy
                && !self.assistant_state.unstarted_threads.contains(thread_id)
                && !self.assistant_state.regenerating_titles.contains(thread_id),
            rename: Rc::new(on_qrow(handle, {
                let id = id.clone();
                move |this, _: &ClickEvent, window, cx| {
                    this.begin_assistant_rename(&id, window, cx);
                }
            })),
            regenerate: Rc::new(on_qrow(handle, {
                let id = id.clone();
                move |this, _: &ClickEvent, _, cx| this.regenerate_assistant_title(&id, cx)
            })),
            delete: Rc::new(on_qrow(handle, move |this, _: &ClickEvent, window, cx| {
                this.confirm_assistant_delete(id.clone(), window, cx);
            })),
        }
    }

    /// Shows a conversation. A closed conversation stays detached until its
    /// next message needs a query tab.
    pub(super) fn select_assistant_thread(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog_open() {
            return;
        }
        if self.assistant.conversation(id).is_none() {
            return;
        }
        if let Some(index) = self.thread_tab_index(id) {
            self.activate(index, window, cx);
        } else {
            self.assistant_state.browsed_thread = Some(id.to_owned());
            self.show_assistant_composer(Some(ComposerTarget::Detached(id.to_owned())), window, cx);
            if self.assistant_state.open {
                self.thread_run_mut(id).unread = None;
            }
            self.load_assistant_thread(id, cx);
        }
        self.assistant_pane
            .update(cx, |pane, cx| pane.thread_selected(cx));
        self.scroll_assistant_to_bottom(cx);
        self.focus_assistant_composer(window, cx);
        self.changed(cx);
    }

    /// Asks Codex for a title while the conversation still has the temporary title.
    pub(super) fn request_assistant_title(&mut self, thread_id: &str, cx: &mut Context<Self>) {
        if self.assistant.conversations.iter().any(|conversation| {
            conversation.thread_id == thread_id
                && conversation.title_source == AssistantTitleSource::Temporary
        }) && !self.assistant_state.pending_titles.contains(thread_id)
        {
            self.send_assistant_title_request(thread_id, cx);
        }
    }

    pub(in crate::ui) fn assistant_title_generating(&self, thread_id: &str) -> bool {
        self.assistant_state.pending_titles.contains(thread_id)
            || self.assistant_state.regenerating_titles.contains(thread_id)
    }

    /// Sends the loaded messages of a conversation to Codex for a title.
    /// Returns false when the conversation has no user message or Codex is
    /// not connected.
    pub(super) fn send_assistant_title_request(
        &mut self,
        thread_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let messages: Vec<_> = self
            .assistant_state
            .transcripts
            .get(thread_id)
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry.speaker {
                Speaker::User => Some(("user", entry.text().to_string())),
                Speaker::Assistant => Some(("assistant", entry.text().to_string())),
                Speaker::Activity | Speaker::Error => None,
            })
            .collect();
        if !messages.iter().any(|(role, _)| *role == "user") {
            return false;
        }
        let model = self.assistant_state.snapshot.as_ref().and_then(|snapshot| {
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
            self.assistant_state
                .pending_titles
                .insert(thread_id.to_owned());
            cx.notify();
        }
        requested
    }
}
