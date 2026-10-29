//! Conversation transcripts that Qrow keeps on this computer. A saved
//! conversation shows from its file, so that browsing does not start or wake
//! the harness.
//!
//! Each conversation has one JSON Lines file: a header line, then one line
//! for each entry, oldest first. A writer thread replaces the whole file
//! atomically, so a crash leaves the old or the new file.

use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, mpsc},
    thread::{self, JoinHandle},
    time::Duration,
};
use uuid::Uuid;

pub const TRANSCRIPT_VERSION: u32 = 1;
/// A larger transcript loses its oldest entries when Qrow saves it.
pub const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024 * 1024;
/// A larger entry keeps the start of its text and detail, so that one
/// entry cannot push all others out of the file.
pub const MAX_ENTRY_BYTES: usize = 1024 * 1024;
const TRUNCATION_NOTE: &str = "\n\n[Qrow saved only the start of this text.]";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoredSpeaker {
    User,
    Assistant,
    Activity,
    Error,
}

/// The state of a tool card. A card that was still running when Qrow saved
/// it keeps its step.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoredToolState {
    Running {
        step: String,
    },
    Done {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outcome: Option<String>,
    },
    QueryResult {
        rows: usize,
        more: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        elapsed_ms: Option<u64>,
    },
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredTool {
    /// The canonical Qrow tool name, for example `query-run`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub state: StoredToolState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StoredEntry {
    pub id: Uuid,
    pub speaker: StoredSpeaker,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<StoredTool>,
    /// The text of an expanded tool card: its query tab, arguments, and result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A saved conversation: its entries, oldest first, and the harness cursor
/// of the history before the first entry.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoredTranscript {
    pub entries: Vec<StoredEntry>,
    pub older_cursor: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Header {
    version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    older_cursor: Option<String>,
}

/// The transcript file of `thread_id` in `directory`. An identifier that is
/// not safe as a file name has no file.
pub fn transcript_path(directory: &Path, thread_id: &str) -> Option<PathBuf> {
    let safe = !thread_id.is_empty()
        && thread_id.len() <= 128
        && !thread_id.starts_with('.')
        && thread_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
    safe.then(|| directory.join(format!("{thread_id}.jsonl")))
}

/// Encodes a transcript. When it is larger than `MAX_TRANSCRIPT_BYTES`,
/// the oldest entries do not go in the file, and the file has no older
/// cursor, because the entries no longer follow it.
pub fn encode(transcript: &StoredTranscript) -> Vec<u8> {
    encode_with_limit(transcript, MAX_TRANSCRIPT_BYTES)
}

/// The entry with at most `MAX_ENTRY_BYTES` of text and detail.
fn bounded(entry: &StoredEntry) -> Cow<'_, StoredEntry> {
    let size = entry.text.len() + entry.detail.as_ref().map_or(0, String::len);
    if size <= MAX_ENTRY_BYTES {
        return Cow::Borrowed(entry);
    }
    let cut = |text: &str| {
        let mut end = MAX_ENTRY_BYTES / 2;
        if text.len() <= end {
            return text.to_owned();
        }
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}{TRUNCATION_NOTE}", &text[..end])
    };
    let mut entry = entry.clone();
    entry.text = cut(&entry.text);
    entry.detail = entry.detail.as_deref().map(cut);
    Cow::Owned(entry)
}

fn encode_with_limit(transcript: &StoredTranscript, limit: usize) -> Vec<u8> {
    let header = |older_cursor: Option<String>| {
        let mut header = serde_json::to_vec(&Header {
            version: TRANSCRIPT_VERSION,
            older_cursor,
        })
        .expect("transcript header encodes");
        header.push(b'\n');
        header
    };
    let full_header = header(transcript.older_cursor.clone());
    let mut lines = VecDeque::new();
    let mut size = full_header.len();
    for entry in transcript.entries.iter().rev() {
        let Ok(mut line) = serde_json::to_vec(&bounded(entry)) else {
            continue;
        };
        line.push(b'\n');
        if size + line.len() > limit {
            break;
        }
        size += line.len();
        lines.push_front(line);
    }
    let header = if lines.len() == transcript.entries.len() {
        full_header
    } else {
        header(None)
    };
    let mut bytes = Vec::with_capacity(header.len() + size);
    bytes.extend_from_slice(&header);
    for line in lines {
        bytes.extend_from_slice(&line);
    }
    bytes
}

/// Decodes a transcript file. Returns `None` for a file of another version.
/// A damaged line does not stop the other entries.
pub fn decode(bytes: &[u8]) -> Option<StoredTranscript> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    let header: Header = serde_json::from_slice(lines.next()?).ok()?;
    (header.version == TRANSCRIPT_VERSION).then(|| StoredTranscript {
        entries: lines
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect(),
        older_cursor: header.older_cursor,
    })
}

/// Reads the transcript of `thread_id`. `Ok(None)` means that Qrow has no
/// transcript of the conversation. Run it on a background thread.
pub fn load(directory: &Path, thread_id: &str) -> std::io::Result<Option<StoredTranscript>> {
    let Some(path) = transcript_path(directory, thread_id) else {
        return Ok(None);
    };
    match std::fs::read(path) {
        Ok(bytes) => Ok(decode(&bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

enum Job {
    Save(String, PathBuf, Vec<u8>),
    Delete(String, PathBuf),
    Flush(mpsc::Sender<()>),
}

/// The conversations whose last save failed.
type Failed = Arc<Mutex<BTreeSet<String>>>;

/// Saves and deletes transcript files on a writer thread, in order. The
/// window thread does not wait for the disk.
pub struct TranscriptStore {
    directory: PathBuf,
    jobs: Option<mpsc::Sender<Job>>,
    writer: Option<JoinHandle<()>>,
    failed: Failed,
}

impl TranscriptStore {
    pub fn new(directory: PathBuf) -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::channel();
        let writer_directory = directory.clone();
        let failed = Failed::default();
        let writer_failed = Arc::clone(&failed);
        let writer = thread::Builder::new()
            .name("qrow-transcripts".into())
            .spawn(move || write_jobs(&writer_directory, &receiver, &writer_failed))?;
        Ok(Self {
            directory,
            jobs: Some(jobs),
            writer: Some(writer),
            failed,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn save(&self, thread_id: &str, bytes: Vec<u8>) {
        if let Some(path) = transcript_path(&self.directory, thread_id) {
            self.send(Job::Save(thread_id.to_owned(), path, bytes));
        }
    }

    pub fn delete(&self, thread_id: &str) {
        if let Some(path) = transcript_path(&self.directory, thread_id) {
            self.send(Job::Delete(thread_id.to_owned(), path));
        }
    }

    /// The conversations whose last save failed since the previous call.
    /// Save them again.
    pub fn take_failed(&self) -> Vec<String> {
        std::mem::take(&mut *self.failed.lock().unwrap_or_else(PoisonError::into_inner))
            .into_iter()
            .collect()
    }

    /// Waits at most `timeout` until the writer has done the earlier jobs.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (done, wait) = mpsc::channel();
        self.send(Job::Flush(done));
        wait.recv_timeout(timeout).is_ok()
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }
}

impl Drop for TranscriptStore {
    fn drop(&mut self) {
        self.jobs.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

fn write_jobs(directory: &Path, receiver: &mpsc::Receiver<Job>, failed: &Failed) {
    while let Ok(job) = receiver.recv() {
        // Only the last job for a file counts. A burst of saves writes once.
        let mut batch = vec![job];
        batch.extend(receiver.try_iter());
        let mut flushes = Vec::new();
        let mut last: Vec<(String, PathBuf, Option<Vec<u8>>)> = Vec::new();
        for job in batch {
            let (thread, path, bytes) = match job {
                Job::Save(thread, path, bytes) => (thread, path, Some(bytes)),
                Job::Delete(thread, path) => (thread, path, None),
                Job::Flush(done) => {
                    flushes.push(done);
                    continue;
                }
            };
            last.retain(|(_, earlier, _)| *earlier != path);
            last.push((thread, path, bytes));
        }
        for (thread, path, bytes) in last {
            let result = match bytes {
                Some(bytes) => std::fs::create_dir_all(directory)
                    .map_err(anyhow::Error::from)
                    .and_then(|()| crate::storage::write_atomically(&path, &bytes)),
                None => match std::fs::remove_file(&path) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
                    _ => Ok(()),
                },
            };
            let mut failed = failed.lock().unwrap_or_else(PoisonError::into_inner);
            match result {
                Ok(()) => {
                    failed.remove(&thread);
                }
                Err(error) => {
                    eprintln!("Could not save an assistant transcript: {error:#}");
                    failed.insert(thread);
                }
            }
        }
        for done in flushes {
            let _ = done.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(speaker: StoredSpeaker, text: &str) -> StoredEntry {
        StoredEntry {
            id: Uuid::new_v4(),
            speaker,
            text: text.into(),
            turn_id: Some("turn-1".into()),
            tool: None,
            detail: None,
        }
    }

    fn every_kind() -> Vec<StoredEntry> {
        let tool = |name: &str, state| StoredEntry {
            tool: Some(StoredTool {
                name: name.into(),
                target: Some("Query 1".into()),
                state,
            }),
            detail: Some(format!(
                "Query tab: Query 1\nArguments:\n{{}}\nResult:\n{name}"
            )),
            ..entry(StoredSpeaker::Activity, name)
        };
        vec![
            entry(StoredSpeaker::User, "Count paid bookings"),
            tool(
                "tab-append-sql",
                StoredToolState::Done {
                    outcome: Some("Appended".into()),
                },
            ),
            tool(
                "query-run",
                StoredToolState::QueryResult {
                    rows: 500,
                    more: true,
                    elapsed_ms: Some(1250),
                },
            ),
            tool("query-read-logs", StoredToolState::Failed),
            tool("query-run", StoredToolState::Cancelled),
            tool(
                "query-run",
                StoredToolState::Running {
                    step: "Executing".into(),
                },
            ),
            entry(
                StoredSpeaker::Assistant,
                "There are **42** bookings.\n\n| a |\n|---|",
            ),
            entry(StoredSpeaker::Error, "The turn failed."),
        ]
    }

    fn transcript(entries: &[StoredEntry]) -> StoredTranscript {
        StoredTranscript {
            entries: entries.to_vec(),
            older_cursor: None,
        }
    }

    #[test]
    fn every_entry_kind_round_trips() {
        let saved = StoredTranscript {
            entries: every_kind(),
            older_cursor: Some("cursor-1".into()),
        };
        assert_eq!(decode(&encode(&saved)), Some(saved));
    }

    #[test]
    fn damaged_lines_skip_and_other_versions_do_not_load() {
        let entries = every_kind();
        let mut bytes = encode(&transcript(&entries));
        bytes.extend_from_slice(b"{\"id\":\n");
        bytes.extend_from_slice(b"not json\n");
        assert_eq!(decode(&bytes), Some(transcript(&entries)));
        assert_eq!(decode(b"{\"version\":99}\n"), None);
        assert_eq!(decode(b""), None);
    }

    #[test]
    fn a_large_transcript_keeps_its_newest_entries_without_a_cursor() {
        let entries: Vec<_> = (0..10)
            .map(|index| entry(StoredSpeaker::User, &format!("{index:0>100}")))
            .collect();
        let line = serde_json::to_vec(&entries[0]).unwrap().len() + 1;
        let saved = StoredTranscript {
            entries: entries.clone(),
            older_cursor: Some("cursor-1".into()),
        };
        let bytes = encode_with_limit(&saved, 40 + 3 * line);
        // The cursor does not follow the oldest saved entry any more.
        assert_eq!(decode(&bytes), Some(transcript(&entries[7..])));
    }

    #[test]
    fn a_huge_entry_keeps_its_start_and_the_other_entries() {
        let mut entries = every_kind();
        let huge = "é".repeat(MAX_TRANSCRIPT_BYTES);
        entries.push(StoredEntry {
            detail: Some(huge.clone()),
            ..entry(StoredSpeaker::Assistant, &huge)
        });
        let loaded = decode(&encode(&transcript(&entries))).unwrap().entries;
        assert_eq!(loaded.len(), entries.len());
        assert_eq!(loaded[..entries.len() - 1], entries[..entries.len() - 1]);
        let last = loaded.last().unwrap();
        assert!(last.text.starts_with("éé") && last.text.ends_with(TRUNCATION_NOTE));
        assert!(last.text.len() + last.detail.as_ref().unwrap().len() <= MAX_ENTRY_BYTES + 200);
    }

    #[test]
    fn only_safe_identifiers_have_files() {
        let directory = Path::new("/transcripts");
        assert_eq!(
            transcript_path(directory, "019a2b3c-thread_1.x"),
            Some(directory.join("019a2b3c-thread_1.x.jsonl"))
        );
        for unsafe_id in ["", "../x", "a/b", ".hidden", "a b", &"x".repeat(129)] {
            assert_eq!(transcript_path(directory, unsafe_id), None, "{unsafe_id}");
        }
    }

    #[test]
    fn the_store_saves_the_last_version_and_deletes() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join("transcripts");
        let store = TranscriptStore::new(folder.clone()).unwrap();
        let entries = every_kind();
        store.save("thread-1", encode(&transcript(&entries[..1])));
        store.save("thread-1", encode(&transcript(&entries)));
        store.save("thread-2", encode(&transcript(&entries[..2])));
        assert!(store.flush(Duration::from_secs(5)));
        assert_eq!(
            load(&folder, "thread-1").unwrap(),
            Some(transcript(&entries))
        );
        assert_eq!(load(&folder, "thread-2").unwrap().unwrap().entries.len(), 2);
        assert!(store.take_failed().is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(folder.join("thread-1.jsonl"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        store.delete("thread-1");
        store.delete("missing");
        drop(store);
        assert_eq!(load(&folder, "thread-1").unwrap(), None);
        assert!(load(&folder, "thread-2").unwrap().is_some());
    }

    #[test]
    fn a_failed_save_is_reported_until_a_save_succeeds() {
        let directory = tempfile::tempdir().unwrap();
        // A file in place of the folder makes each save fail.
        let folder = directory.path().join("transcripts");
        std::fs::write(&folder, "").unwrap();
        let store = TranscriptStore::new(folder.clone()).unwrap();
        store.save("thread-1", encode(&transcript(&every_kind())));
        assert!(store.flush(Duration::from_secs(5)));
        assert_eq!(store.take_failed(), ["thread-1"]);
        assert!(store.take_failed().is_empty());

        std::fs::remove_file(&folder).unwrap();
        store.save("thread-1", encode(&transcript(&every_kind())));
        assert!(store.flush(Duration::from_secs(5)));
        assert!(store.take_failed().is_empty());
        assert!(load(&folder, "thread-1").unwrap().is_some());
    }
}
