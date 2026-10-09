//! Conversation transcripts that Qrow keeps on this computer. A saved
//! conversation shows from its file, so that browsing does not start or wake
//! the harness.
//!
//! Each conversation has one JSON Lines file: a header line, then one line
//! for each entry, oldest first. A writer thread replaces the whole file
//! atomically, so a crash leaves the old or the new file.

use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::Duration,
};
use uuid::Uuid;

pub const TRANSCRIPT_VERSION: u32 = 1;
/// A larger transcript loses its oldest entries when Qrow saves it.
pub const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024 * 1024;

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

#[derive(Deserialize, Serialize)]
struct Header {
    version: u32,
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

/// Encodes the entries. When they are larger than `MAX_TRANSCRIPT_BYTES`,
/// the oldest entries do not go in the file.
pub fn encode(entries: &[StoredEntry]) -> Vec<u8> {
    encode_with_limit(entries, MAX_TRANSCRIPT_BYTES)
}

fn encode_with_limit(entries: &[StoredEntry], limit: usize) -> Vec<u8> {
    let mut header = serde_json::to_vec(&Header {
        version: TRANSCRIPT_VERSION,
    })
    .expect("transcript header encodes");
    header.push(b'\n');
    let mut lines = VecDeque::new();
    let mut size = header.len();
    for entry in entries.iter().rev() {
        let Ok(mut line) = serde_json::to_vec(entry) else {
            continue;
        };
        line.push(b'\n');
        if size + line.len() > limit {
            break;
        }
        size += line.len();
        lines.push_front(line);
    }
    let mut bytes = Vec::with_capacity(size);
    bytes.extend_from_slice(&header);
    for line in lines {
        bytes.extend_from_slice(&line);
    }
    bytes
}

/// Decodes a transcript file. Returns `None` for a file of another version.
/// A damaged line does not stop the other entries.
pub fn decode(bytes: &[u8]) -> Option<Vec<StoredEntry>> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    let header: Header = serde_json::from_slice(lines.next()?).ok()?;
    (header.version == TRANSCRIPT_VERSION).then(|| {
        lines
            .filter(|line| !line.is_empty())
            .filter_map(|line| serde_json::from_slice(line).ok())
            .collect()
    })
}

/// Reads the transcript of `thread_id`. `Ok(None)` means that Qrow has no
/// transcript of the conversation. Run it on a background thread.
pub fn load(directory: &Path, thread_id: &str) -> std::io::Result<Option<Vec<StoredEntry>>> {
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
    Save(PathBuf, Vec<u8>),
    Delete(PathBuf),
    Flush(mpsc::Sender<()>),
}

/// Saves and deletes transcript files on a writer thread, in order. The
/// window thread does not wait for the disk.
pub struct TranscriptStore {
    directory: PathBuf,
    jobs: Option<mpsc::Sender<Job>>,
    writer: Option<JoinHandle<()>>,
}

impl TranscriptStore {
    pub fn new(directory: PathBuf) -> std::io::Result<Self> {
        let (jobs, receiver) = mpsc::channel();
        let writer_directory = directory.clone();
        let writer = thread::Builder::new()
            .name("qrow-transcripts".into())
            .spawn(move || write_jobs(&writer_directory, &receiver))?;
        Ok(Self {
            directory,
            jobs: Some(jobs),
            writer: Some(writer),
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn save(&self, thread_id: &str, bytes: Vec<u8>) {
        if let Some(path) = transcript_path(&self.directory, thread_id) {
            self.send(Job::Save(path, bytes));
        }
    }

    pub fn delete(&self, thread_id: &str) {
        if let Some(path) = transcript_path(&self.directory, thread_id) {
            self.send(Job::Delete(path));
        }
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

fn write_jobs(directory: &Path, receiver: &mpsc::Receiver<Job>) {
    while let Ok(job) = receiver.recv() {
        // Only the last job for a file counts. A burst of saves writes once.
        let mut batch = vec![job];
        batch.extend(receiver.try_iter());
        let mut flushes = Vec::new();
        let mut last: Vec<(PathBuf, Option<Vec<u8>>)> = Vec::new();
        for job in batch {
            let (path, bytes) = match job {
                Job::Save(path, bytes) => (path, Some(bytes)),
                Job::Delete(path) => (path, None),
                Job::Flush(done) => {
                    flushes.push(done);
                    continue;
                }
            };
            last.retain(|(earlier, _)| *earlier != path);
            last.push((path, bytes));
        }
        for (path, bytes) in last {
            match bytes {
                Some(bytes) => {
                    if std::fs::create_dir_all(directory).is_ok()
                        && let Err(error) = crate::storage::write_atomically(&path, &bytes)
                    {
                        eprintln!("Could not save an assistant transcript: {error:#}");
                    }
                }
                None => match std::fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => eprintln!("Could not delete an assistant transcript: {error}"),
                },
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

    #[test]
    fn every_entry_kind_round_trips() {
        let entries = every_kind();
        assert_eq!(decode(&encode(&entries)), Some(entries));
    }

    #[test]
    fn damaged_lines_skip_and_other_versions_do_not_load() {
        let entries = every_kind();
        let mut bytes = encode(&entries);
        bytes.extend_from_slice(b"{\"id\":\n");
        bytes.extend_from_slice(b"not json\n");
        assert_eq!(decode(&bytes), Some(entries));
        assert_eq!(decode(b"{\"version\":99}\n"), None);
        assert_eq!(decode(b""), None);
    }

    #[test]
    fn a_large_transcript_keeps_its_newest_entries() {
        let entries: Vec<_> = (0..10)
            .map(|index| entry(StoredSpeaker::User, &format!("{index:0>100}")))
            .collect();
        let line = serde_json::to_vec(&entries[0]).unwrap().len() + 1;
        let bytes = encode_with_limit(&entries, 20 + 3 * line);
        assert_eq!(decode(&bytes).unwrap(), entries[7..]);
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
        store.save("thread-1", encode(&entries[..1]));
        store.save("thread-1", encode(&entries));
        store.save("thread-2", encode(&entries[..2]));
        assert!(store.flush(Duration::from_secs(5)));
        assert_eq!(load(&folder, "thread-1").unwrap(), Some(entries));
        assert_eq!(load(&folder, "thread-2").unwrap().unwrap().len(), 2);
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
}
