//! The transcripts that Qrow keeps on this computer. A conversation shows
//! from its file, so that browsing does not start or wake the harness.
use super::*;
use crate::assistant::transcripts::{
    self, StoredEntry, StoredSpeaker, StoredTool, StoredToolState, StoredTranscript,
    TranscriptStore,
};
use std::hash::{DefaultHasher, Hash, Hasher};

/// The time that Qrow waits after a change before it saves a transcript.
/// The end of a turn saves at once.
const SAVE_DELAY: Duration = Duration::from_secs(1);

/// What Qrow knows about the saved transcript of a conversation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum LocalTranscript {
    /// The file is being read.
    Loading,
    /// The transcript in memory includes the file. `saved` is the
    /// fingerprint of the entries in the file.
    Loaded { saved: u64 },
    /// There is no file. The harness history fills the transcript, and Qrow
    /// then saves it.
    Missing,
    /// The file could not be read. Qrow does not replace it.
    Unavailable,
}

impl ToolState {
    fn stored(&self) -> StoredToolState {
        match self {
            Self::Running(step) => StoredToolState::Running { step: step.clone() },
            Self::Done(outcome) => StoredToolState::Done {
                outcome: outcome.clone(),
            },
            Self::QueryResult {
                rows,
                more,
                elapsed,
            } => StoredToolState::QueryResult {
                rows: *rows,
                more: *more,
                elapsed_ms: elapsed.map(|elapsed| elapsed.as_millis() as u64),
            },
            Self::Failed => StoredToolState::Failed,
            Self::Cancelled => StoredToolState::Cancelled,
        }
    }

    /// A card that was still running when Qrow saved it did not end in this
    /// process, so it shows as cancelled.
    fn from_stored(state: StoredToolState) -> Self {
        match state {
            StoredToolState::Running { .. } | StoredToolState::Cancelled => Self::Cancelled,
            StoredToolState::Done { outcome } => Self::Done(outcome),
            StoredToolState::QueryResult {
                rows,
                more,
                elapsed_ms,
            } => Self::QueryResult {
                rows,
                more,
                elapsed: elapsed_ms.map(Duration::from_millis),
            },
            StoredToolState::Failed => Self::Failed,
        }
    }
}

impl TranscriptEntry {
    pub(super) fn stored(&self) -> StoredEntry {
        StoredEntry {
            id: self.id,
            speaker: match self.speaker {
                Speaker::User => StoredSpeaker::User,
                Speaker::Assistant => StoredSpeaker::Assistant,
                Speaker::Activity => StoredSpeaker::Activity,
                Speaker::Error => StoredSpeaker::Error,
            },
            text: self.text().to_string(),
            turn_id: self.turn_id.clone(),
            tool: self.tool.as_ref().map(|tool| StoredTool {
                name: tool.kind.name().to_owned(),
                target: tool.target.clone(),
                state: tool.state.stored(),
            }),
            detail: self.detail.clone(),
        }
    }

    /// The entry as it showed live, complete and collapsed.
    pub(super) fn from_stored(stored: StoredEntry) -> Self {
        let speaker = match stored.speaker {
            StoredSpeaker::User => Speaker::User,
            StoredSpeaker::Assistant => Speaker::Assistant,
            StoredSpeaker::Activity => Speaker::Activity,
            StoredSpeaker::Error => Speaker::Error,
        };
        let mut entry = match stored.tool {
            Some(tool) => TranscriptEntry::tool(
                ToolActivity {
                    kind: ToolKind::from_name(&tool.name),
                    target: tool.target,
                    state: ToolState::from_stored(tool.state),
                },
                stored.turn_id.unwrap_or_default(),
            ),
            None => TranscriptEntry::new(speaker, stored.text, stored.turn_id),
        };
        entry.id = stored.id;
        entry.detail = stored.detail;
        entry
    }
}

/// A fingerprint of the saved content of a transcript. The reveal step and
/// the expanded state of cards are not saved, so they do not count.
fn fingerprint(entries: &[TranscriptEntry], older_cursor: Option<&String>) -> u64 {
    let mut hasher = DefaultHasher::new();
    older_cursor.hash(&mut hasher);
    entries.len().hash(&mut hasher);
    for entry in entries {
        entry.id.hash(&mut hasher);
        (entry.speaker as u8).hash(&mut hasher);
        entry.text().as_str().hash(&mut hasher);
        entry.turn_id.hash(&mut hasher);
        entry.detail.hash(&mut hasher);
        if let Some(tool) = &entry.tool {
            tool.kind.name().hash(&mut hasher);
            tool.target.hash(&mut hasher);
            format!("{:?}", tool.state).hash(&mut hasher);
        }
    }
    hasher.finish()
}

impl Qrow {
    /// Starts to read the saved transcript of a conversation. Returns false
    /// when Qrow keeps no transcripts or already knows this one.
    pub(super) fn load_local_transcript(
        &mut self,
        thread_id: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(store) = self.assistant_state.store.as_ref() else {
            return false;
        };
        if self.assistant_state.local.contains_key(thread_id) {
            return false;
        }
        let directory = store.directory().to_path_buf();
        let thread = thread_id.to_owned();
        self.assistant_state
            .local
            .insert(thread.clone(), LocalTranscript::Loading);
        let read = cx.background_spawn({
            let thread = thread.clone();
            async move { transcripts::load(&directory, &thread) }
        });
        cx.spawn(async move |this, cx| {
            let result = read.await;
            let _ = this.update(cx, |this, cx| {
                this.local_transcript_read(&thread, result, cx)
            });
        })
        .detach();
        true
    }

    fn local_transcript_read(
        &mut self,
        thread_id: &str,
        result: std::io::Result<Option<StoredTranscript>>,
        cx: &mut Context<Self>,
    ) {
        // A delete during the read removed the conversation.
        if self.assistant_state.local.get(thread_id) != Some(&LocalTranscript::Loading) {
            return;
        }
        let state = match result {
            // A file without entries does not hide the harness history.
            Ok(Some(stored)) if !stored.entries.is_empty() => {
                let stored_cursor = stored.older_cursor.clone();
                if let Some(cursor) = stored.older_cursor.clone()
                    && !self.assistant_state.loaded_cursors.contains_key(thread_id)
                {
                    self.assistant_state
                        .older_cursors
                        .entry(thread_id.to_owned())
                        .or_insert(cursor);
                }
                let stored: Vec<_> = stored
                    .entries
                    .into_iter()
                    .map(TranscriptEntry::from_stored)
                    .collect();
                let entries = self
                    .assistant_state
                    .transcripts
                    .entry(thread_id.to_owned())
                    .or_default();
                // Entries of this process, for example a sent message, follow
                // the saved ones.
                let known: BTreeSet<_> = stored.iter().map(|entry| entry.id).collect();
                let newer: Vec<_> = entries
                    .drain(..)
                    .filter(|entry| !known.contains(&entry.id))
                    .collect();
                let saved = fingerprint(&stored, stored_cursor.as_ref());
                entries.extend(stored);
                let unsaved = !newer.is_empty();
                entries.extend(newer);
                if self.displayed_thread().as_deref() == Some(thread_id) {
                    self.scroll_assistant_to_bottom(cx);
                }
                self.sync_assistant_pane(cx);
                LocalTranscript::Loaded {
                    saved: if unsaved { 0 } else { saved },
                }
            }
            Ok(_) => LocalTranscript::Missing,
            Err(error) => {
                eprintln!("Could not read an assistant transcript: {error}");
                LocalTranscript::Unavailable
            }
        };
        self.assistant_state
            .local
            .insert(thread_id.to_owned(), state);
        let titling = self.assistant_state.title_history_reads.contains(thread_id);
        if matches!(state, LocalTranscript::Loaded { .. }) {
            if titling {
                self.assistant_state.title_history_reads.remove(thread_id);
                if !self.send_assistant_title_request(thread_id, cx) {
                    self.assistant_state.regenerating_titles.remove(thread_id);
                    self.assistant_state.notice = Some(AssistantNotice::info(
                        "This conversation has no messages for a title.",
                    ));
                }
            }
        } else if titling {
            // The harness history gives the messages for the title.
            if !self.assistant_command(AssistantCommand::Read(thread_id.to_owned()), cx) {
                self.assistant_state.title_history_reads.remove(thread_id);
                self.assistant_state.regenerating_titles.remove(thread_id);
            }
        } else if self.displayed_thread().as_deref() == Some(thread_id) {
            // The harness history fills the transcript.
            self.load_assistant_thread(thread_id, cx);
        }
        cx.notify();
    }

    /// Whether the transcript of a conversation shows from its file, so the
    /// harness does not need to load it.
    pub(super) fn local_transcript_shows(&self, thread_id: &str) -> bool {
        matches!(
            self.assistant_state.local.get(thread_id),
            Some(LocalTranscript::Loading | LocalTranscript::Loaded { .. })
        )
    }

    /// Marks a conversation whose transcript Qrow saves from now on: a new
    /// conversation, or one whose harness history filled the transcript.
    pub(super) fn track_local_transcript(&mut self, thread_id: &str) {
        if self.assistant_state.store.is_none() {
            return;
        }
        match self.assistant_state.local.get(thread_id) {
            Some(LocalTranscript::Loaded { .. } | LocalTranscript::Unavailable) => {}
            _ => {
                self.assistant_state
                    .local
                    .insert(thread_id.to_owned(), LocalTranscript::Loaded { saved: 0 });
            }
        }
    }

    /// Saves the changed transcripts after a short delay.
    pub(super) fn schedule_transcript_save(&mut self, cx: &mut Context<Self>) {
        if self.assistant_state.store.is_none() || self.assistant_state.save_transcripts.is_some() {
            return;
        }
        self.assistant_state.save_transcripts = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SAVE_DELAY).await;
            let _ = this.update(cx, |this, _| {
                this.assistant_state.save_transcripts = None;
                this.save_transcripts(None);
            });
        }));
    }

    /// Saves the transcripts that changed since Qrow last saved them: all
    /// conversations, or only `thread_id`.
    pub(in crate::ui) fn save_transcripts(&mut self, thread_id: Option<&str>) {
        let Some(store) = self.assistant_state.store.as_ref() else {
            return;
        };
        // A failed save tries again with the current transcript.
        for thread in store.take_failed() {
            if let Some(LocalTranscript::Loaded { saved }) =
                self.assistant_state.local.get_mut(&thread)
            {
                *saved = 0;
            }
        }
        let conversations = &self.assistant;
        let cursors = &self.assistant_state.older_cursors;
        for (thread, state) in &mut self.assistant_state.local {
            if thread_id.is_some_and(|id| id != thread) {
                continue;
            }
            let LocalTranscript::Loaded { saved } = state else {
                continue;
            };
            // A conversation without a turn is not saved.
            if conversations.conversation(thread).is_none() {
                continue;
            }
            let Some(entries) = self.assistant_state.transcripts.get(thread) else {
                continue;
            };
            let current = fingerprint(entries, cursors.get(thread));
            if current == *saved {
                continue;
            }
            let stored = StoredTranscript {
                entries: entries.iter().map(TranscriptEntry::stored).collect(),
                older_cursor: cursors.get(thread).cloned(),
            };
            store.save(thread, transcripts::encode(&stored));
            *saved = current;
        }
    }

    /// Removes the saved transcript of a deleted conversation.
    pub(super) fn delete_local_transcript(&mut self, thread_id: &str) {
        self.assistant_state.local.remove(thread_id);
        if let Some(store) = &self.assistant_state.store {
            store.delete(thread_id);
        }
    }
}

/// The transcript store of a window with a saved workspace.
pub(in crate::ui) fn transcript_store(
    workspace: Option<&std::path::Path>,
) -> Option<TranscriptStore> {
    let directory = crate::storage::assistant_transcripts_directory(workspace?);
    TranscriptStore::new(directory)
        .map_err(|error| eprintln!("Could not start the transcript writer: {error}"))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn stored_entries_show_as_they_did_live() {
        let mut card = TranscriptEntry::tool(
            ToolActivity {
                kind: ToolKind::RunQuery,
                target: Some("Query 1".into()),
                state: ToolState::QueryResult {
                    rows: 42,
                    more: false,
                    elapsed: Some(Duration::from_millis(1250)),
                },
            },
            "turn-1".into(),
        )
        .with_detail("Query tab: Query 1\nResult:\n42 rows".into());
        card.toggle_expanded();
        // The live reply has shown all of its streamed text.
        let mut reply =
            TranscriptEntry::streamed(Speaker::Assistant, "There are 42.", Some("turn-1".into()));
        reply.show_all();
        let live = vec![
            TranscriptEntry::new(Speaker::User, "Count paid bookings", Some("turn-1".into())),
            card,
            reply,
            TranscriptEntry::new(Speaker::Error, "The turn failed.", Some("turn-1".into())),
        ];
        let loaded: Vec<_> = live
            .iter()
            .map(|entry| TranscriptEntry::from_stored(entry.stored()))
            .collect();
        assert_eq!(fingerprint(&live, None), fingerprint(&loaded, None));
        let cursor = "cursor-1".to_owned();
        assert_ne!(fingerprint(&live, None), fingerprint(&live, Some(&cursor)));
        for (live, loaded) in live.iter().zip(&loaded) {
            assert_eq!(live.id, loaded.id);
            assert_eq!(live.label(), loaded.label());
            assert_eq!(live.text(), loaded.text());
            assert_eq!(live.detail, loaded.detail);
            // A loaded entry shows complete and collapsed.
            assert_eq!(loaded.shown_text(), loaded.text());
            assert!(!loaded.revealing() && !loaded.expanded());
        }
    }

    #[::core::prelude::v1::test]
    fn a_card_that_did_not_end_loads_as_cancelled() {
        let running = TranscriptEntry::tool(
            ToolActivity {
                kind: ToolKind::RunQuery,
                target: None,
                state: ToolState::Running("Executing".into()),
            },
            "turn-1".into(),
        );
        let loaded = TranscriptEntry::from_stored(running.stored());
        assert_eq!(
            loaded.tool.map(|tool| tool.state),
            Some(ToolState::Cancelled)
        );
    }
}
