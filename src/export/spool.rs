//! Anonymous result storage with independent readers and committed records.
use super::{
    Context, DiskWriter, Rows,
    budget::{self, Allocation, Allowance},
    check_cancelled, temporal,
    value::{self, Kind, Value},
};
use crate::model::{Column, Row};
use serde::{
    Deserialize, Serialize,
    ser::{Error as _, SerializeSeq},
};
use std::{
    fs::File,
    io::{self, BufWriter, Write},
    os::unix::fs::FileExt,
    sync::{Arc, Condvar, Mutex, atomic::AtomicBool},
    time::Duration,
};

const MAGIC: &[u8; 8] = b"QROWSP\0\x01";
const MAX_COLUMNS: usize = 4096;
const MAX_BATCH_ROWS: usize = 1000;
pub(crate) const PRODUCER_MEMORY: usize = 128 * budget::MIB;
const READER_MEMORY: usize = 160 * budget::MIB;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Downloading,
    Complete { rows: u64 },
    Failed(String),
    Cancelled,
}

struct Commit {
    offset: u64,
    rows: u64,
    status: Status,
}

#[derive(Serialize)]
struct Header<'a> {
    columns: &'a [Column],
    kinds: Vec<Kind>,
    context: &'a Context,
}

pub struct Spool {
    file: File,
    columns: Vec<Column>,
    context: Context,
    first_record: u64,
    commit: Mutex<Commit>,
    changed: Condvar,
    _metadata: Allocation,
    budget: Arc<budget::Budget>,
}

/// Only the cursor owner receives the producer. It cannot be cloned.
pub struct Producer {
    spool: Arc<Spool>,
    out: BufWriter<DiskWriter<File>>,
    rows: u64,
    memory: Arc<Allowance>,
    _buffer: Allocation,
}

pub struct Reader {
    spool: Arc<Spool>,
    offset: u64,
    rows: u64,
    ended: bool,
    memory: Arc<Allowance>,
}

/// Rows keep their accounting guard until the consumer releases the batch.
pub struct Batch {
    rows: Rows,
}

impl Batch {
    pub fn rows(&self) -> &Rows {
        &self.rows
    }
}

impl Spool {
    pub fn new(columns: &[Column], context: &Context) -> io::Result<(Arc<Self>, Producer)> {
        Self::new_prepared(columns, context, budget::GLOBAL.allowance(PRODUCER_MEMORY)?)
    }

    pub(crate) fn new_prepared(
        columns: &[Column],
        context: &Context,
        memory: Arc<Allowance>,
    ) -> io::Result<(Arc<Self>, Producer)> {
        Self::new_with_memory(columns, context, budget::GLOBAL.clone(), memory)
    }

    #[cfg(test)]
    fn new_in(
        columns: &[Column],
        context: &Context,
        budget: Arc<budget::Budget>,
    ) -> io::Result<(Arc<Self>, Producer)> {
        let memory = budget.allowance(PRODUCER_MEMORY)?;
        Self::new_with_memory(columns, context, budget, memory)
    }

    fn new_with_memory(
        columns: &[Column],
        context: &Context,
        budget: Arc<budget::Budget>,
        memory: Arc<Allowance>,
    ) -> io::Result<(Arc<Self>, Producer)> {
        if columns.is_empty() || columns.len() > MAX_COLUMNS {
            return Err(invalid("Invalid spool column count."));
        }
        let metadata_bytes = columns
            .len()
            .checked_mul(std::mem::size_of::<Column>() + std::mem::size_of::<Kind>())
            .and_then(|bytes| {
                bytes.checked_add(
                    columns
                        .iter()
                        .map(|column| column.name.len() + column.data_type.len())
                        .sum::<usize>(),
                )
            })
            .ok_or_else(|| invalid("Spool schema is too large."))?;
        if metadata_bytes > budget::MAX_ROW_BYTES {
            return Err(invalid("Spool schema exceeds 16 MiB."));
        }
        let context_bytes = context.postgres.as_ref().map_or(0, |context| {
            context.date_style.len() + context.interval_style.len() + context.time_zone.len()
        });
        let metadata = budget.acquire(metadata_bytes + context_bytes + 512)?;
        let buffer = memory.acquire(8192)?;
        let file = tempfile::tempfile()?;
        let mut out = BufWriter::new(DiskWriter::new(file.try_clone()?, &std::env::temp_dir())?);
        out.write_all(MAGIC)?;
        let header = Header {
            columns,
            kinds: columns
                .iter()
                .map(|column| Kind::of(&column.data_type))
                .collect(),
            context,
        };
        let length = count(&header)?;
        let _encoded = memory.acquire(length)?;
        let mut bytes = vec![0; length];
        postcard::to_slice(&header, &mut bytes).map_err(codec)?;
        out.write_all(&(length as u32).to_le_bytes())?;
        out.write_all(&bytes)?;
        out.flush()?;
        let first_record = 12 + length as u64;
        let spool = Arc::new(Self {
            file,
            columns: columns.to_vec(),
            context: context.clone(),
            first_record,
            commit: Mutex::new(Commit {
                offset: first_record,
                rows: 0,
                status: Status::Downloading,
            }),
            changed: Condvar::new(),
            _metadata: metadata,
            budget,
        });
        let producer = Producer {
            spool: spool.clone(),
            out,
            rows: 0,
            memory,
            _buffer: buffer,
        };
        Ok((spool, producer))
    }

    pub fn columns(&self) -> &[Column] {
        &self.columns
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub fn status(&self) -> Status {
        self.commit.lock().unwrap().status.clone()
    }
    pub fn bytes(&self) -> u64 {
        self.commit.lock().unwrap().offset
    }
    pub fn row_count(&self) -> u64 {
        self.commit.lock().unwrap().rows
    }

    pub fn cancel(&self) {
        self.stop(Status::Cancelled);
    }
    pub fn fail(&self, message: impl Into<String>) {
        self.stop(Status::Failed(message.into()));
    }

    fn stop(&self, status: Status) {
        let mut commit = self.commit.lock().unwrap();
        // A complete download belongs to writers and replay. A late writer
        // error cannot change its state or cancel a later query.
        if commit.status == Status::Downloading {
            commit.status = status;
            self.changed.notify_all();
        }
    }

    pub fn reader(self: &Arc<Self>) -> io::Result<Reader> {
        let mut magic = [0; 8];
        self.file.read_exact_at(&mut magic, 0)?;
        if &magic != MAGIC {
            return Err(invalid("Unsupported spool version."));
        }
        Ok(Reader {
            spool: self.clone(),
            offset: self.first_record,
            rows: 0,
            ended: false,
            memory: self.budget.allowance(READER_MEMORY)?,
        })
    }
}

impl Producer {
    pub(crate) fn reserve_fetch(&self) -> io::Result<Allocation> {
        self.memory.acquire(64 * budget::MIB)
    }

    pub fn append(&mut self, rows: &[Row], cancel: &AtomicBool) -> io::Result<()> {
        let result = self.append_rows(rows, cancel);
        self.record_failure(&result, cancel);
        result
    }

    fn append_rows(&mut self, rows: &[Row], cancel: &AtomicBool) -> io::Result<()> {
        for chunk in rows.chunks(MAX_BATCH_ROWS) {
            check_cancelled(cancel)?;
            for (index, row) in chunk.iter().enumerate() {
                if row.len() != self.spool.columns.len() {
                    return Err(invalid("Spool row does not match its columns."));
                }
                budget::row_bytes(row)?;
                for (column, raw) in self.spool.columns.iter().zip(row) {
                    let _normalization =
                        self.memory.acquire(raw.as_ref().map_or(0, String::len))?;
                    value::normalize(Kind::of(&column.data_type), raw.as_deref()).map_err(
                        |error| {
                            io::Error::new(
                                error.kind(),
                                format!(
                                    "Column {}, row {}, value {:?}: {error}",
                                    column.name,
                                    self.rows + index as u64 + 1,
                                    raw.as_ref()
                                        .map(|raw| raw.chars().take(100).collect::<String>())
                                ),
                            )
                        },
                    )?;
                }
            }
            // Split a wide response before serialization. The source stays
            // borrowed; only one bounded record is encoded at a time.
            self.append_bounded(chunk, cancel)?;
        }
        Ok(())
    }

    fn append_bounded(&mut self, rows: &[Row], cancel: &AtomicBool) -> io::Result<()> {
        let record = Record::Batch(BatchRef {
            rows,
            columns: &self.spool.columns,
            context: &self.spool.context,
            memory: &self.memory,
            cancel,
        });
        let length = match count(&record) {
            Ok(length) => length,
            Err(_) if rows.len() > 1 => {
                let middle = rows.len() / 2;
                self.append_bounded(&rows[..middle], cancel)?;
                return self.append_bounded(&rows[middle..], cancel);
            }
            Err(error) => return Err(error),
        };
        let decoded = rows
            .len()
            .saturating_mul(
                std::mem::size_of::<Row>()
                    + self.spool.columns.len() * std::mem::size_of::<Option<String>>(),
            )
            .saturating_add(length + 1024)
            .saturating_add(self.spool.context.postgres.as_ref().map_or(0, |context| {
                context.date_style.len() + context.interval_style.len() + context.time_zone.len()
            }));
        if decoded > budget::MAX_RECORD_BYTES {
            if rows.len() > 1 {
                let middle = rows.len() / 2;
                self.append_bounded(&rows[..middle], cancel)?;
                return self.append_bounded(&rows[middle..], cancel);
            }
            return Err(invalid("Spool decoded batch exceeds 32 MiB."));
        }
        let _encoded = self.memory.acquire(length)?;
        let mut bytes = vec![0; length];
        postcard::to_slice(&record, &mut bytes).map_err(codec)?;
        self.rows = self
            .rows
            .checked_add(rows.len() as u64)
            .ok_or_else(|| invalid("Spool row count overflow."))?;
        self.write_record(&bytes, None, cancel)?;
        Ok(())
    }

    pub fn finish(self, cancel: &AtomicBool) -> io::Result<()> {
        self.finish_with(cancel, || Ok(()))
    }

    /// Hand off operation ownership with publication of the terminal record.
    /// Disk writes finish before this callback runs under the commit lock.
    pub(crate) fn finish_with(
        mut self,
        cancel: &AtomicBool,
        before_commit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let mut bytes = [0; 16];
        let record = Record::End(self.rows);
        let bytes = postcard::to_slice(&record, &mut bytes).map_err(codec)?;
        let result = self.write_record_with(bytes, Some(self.rows), cancel, before_commit);
        self.record_failure(&result, cancel);
        result
    }

    fn record_failure(&self, result: &io::Result<()>, cancel: &AtomicBool) {
        if let Err(error) = result {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                self.spool.cancel();
            } else {
                self.spool.fail(error.to_string());
            }
        }
    }

    fn write_record(
        &mut self,
        bytes: &[u8],
        complete: Option<u64>,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        self.write_record_with(bytes, complete, cancel, || Ok(()))
    }

    fn write_record_with(
        &mut self,
        bytes: &[u8],
        complete: Option<u64>,
        cancel: &AtomicBool,
        before_commit: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        check_cancelled(cancel)?;
        ensure_downloading(&self.spool.commit.lock().unwrap().status)?;
        self.out.write_all(&(bytes.len() as u32).to_le_bytes())?;
        self.out.write_all(bytes)?;
        self.out.flush()?;
        let mut commit = self.spool.commit.lock().unwrap();
        check_cancelled(cancel)?;
        ensure_downloading(&commit.status)?;
        let offset = commit
            .offset
            .checked_add(4 + bytes.len() as u64)
            .ok_or_else(|| invalid("Spool offset overflow."))?;
        before_commit()?;
        commit.offset = offset;
        commit.rows = self.rows;
        if let Some(rows) = complete {
            commit.status = Status::Complete { rows };
        }
        self.spool.changed.notify_all();
        Ok(())
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        self.spool
            .fail("The result download stopped before completion.");
    }
}

impl Reader {
    pub fn next(&mut self, cancel: &AtomicBool) -> io::Result<Option<Batch>> {
        if self.ended {
            return Ok(None);
        }
        let end = {
            let mut commit = self.spool.commit.lock().unwrap();
            loop {
                check_cancelled(cancel)?;
                match &commit.status {
                    Status::Failed(message) => return Err(io::Error::other(message.clone())),
                    Status::Cancelled => return Err(io::Error::other(super::Cancelled)),
                    _ => {}
                }
                if self.offset < commit.offset {
                    break commit.offset;
                }
                if matches!(commit.status, Status::Complete { .. }) {
                    return Err(invalid("Spool is missing its end record."));
                }
                commit = self
                    .spool
                    .changed
                    .wait_timeout(commit, Duration::from_millis(100))
                    .unwrap()
                    .0;
            }
        };
        let mut prefix = [0; 4];
        self.spool.file.read_exact_at(&mut prefix, self.offset)?;
        let length = u32::from_le_bytes(prefix) as usize;
        if length == 0
            || length > budget::MAX_RECORD_BYTES
            || self
                .offset
                .checked_add(4 + length as u64)
                .is_none_or(|next| next > end)
        {
            return Err(invalid("Invalid spool record length."));
        }
        let _encoded = self.memory.acquire(length)?;
        let mut bytes = vec![0; length];
        self.spool.file.read_exact_at(&mut bytes, self.offset + 4)?;
        let (tag, mut remaining) = postcard::take_from_bytes::<u32>(&bytes).map_err(codec)?;
        match tag {
            0 => {
                let (count, rest) = postcard::take_from_bytes::<u64>(remaining).map_err(codec)?;
                remaining = rest;
                if count == 0 || count > MAX_BATCH_ROWS as u64 {
                    return Err(invalid("Invalid spool batch row count."));
                }
                let count = count as usize;
                let allocation = count
                    .checked_mul(
                        std::mem::size_of::<Row>()
                            + self.spool.columns.len() * std::mem::size_of::<Option<String>>(),
                    )
                    .and_then(|metadata| {
                        metadata.checked_add(
                            length
                                + 1024
                                + self.spool.context.postgres.as_ref().map_or(0, |context| {
                                    context.date_style.len()
                                        + context.interval_style.len()
                                        + context.time_zone.len()
                                }),
                        )
                    })
                    .ok_or_else(|| invalid("Spool batch allocation overflow."))?;
                if allocation > budget::MAX_RECORD_BYTES {
                    return Err(invalid("Spool decoded batch exceeds 32 MiB."));
                }
                let memory = self.memory.acquire(allocation)?;
                let mut rows = Vec::with_capacity(count);
                for _ in 0..count {
                    check_cancelled(cancel)?;
                    let (columns, rest) =
                        postcard::take_from_bytes::<u64>(remaining).map_err(codec)?;
                    remaining = rest;
                    if columns != self.spool.columns.len() as u64 {
                        return Err(invalid("Spool row does not match its columns."));
                    }
                    let mut row = Vec::with_capacity(columns as usize);
                    for _ in 0..columns {
                        let (cell, rest) =
                            postcard::take_from_bytes::<Cell<'_>>(remaining).map_err(codec)?;
                        remaining = rest;
                        if cell
                            .raw
                            .is_some_and(|raw| raw.len() > budget::MAX_CELL_BYTES)
                        {
                            return Err(invalid("Spool cell exceeds 8 MiB."));
                        }
                        if matches!(cell.logical, Logical::Null) != cell.raw.is_none() {
                            return Err(invalid("Invalid null spool value."));
                        }
                        row.push(cell.raw.map(str::to_owned));
                    }
                    budget::row_bytes(&row)?;
                    rows.push(row);
                }
                if !remaining.is_empty() {
                    return Err(invalid("Unexpected data after spool batch."));
                }
                self.rows = self
                    .rows
                    .checked_add(count as u64)
                    .ok_or_else(|| invalid("Spool row count overflow."))?;
                self.offset += 4 + length as u64;
                Ok(Some(Batch {
                    rows: Rows::accounted(rows, memory, self.spool.context.clone()),
                }))
            }
            1 => {
                let (rows, rest) = postcard::take_from_bytes::<u64>(remaining).map_err(codec)?;
                let commit = self.spool.commit.lock().unwrap();
                if !rest.is_empty()
                    || rows != self.rows
                    || commit.status != (Status::Complete { rows })
                    || self.offset + 4 + length as u64 != commit.offset
                {
                    return Err(invalid("Spool end record does not match its rows."));
                }
                self.ended = true;
                Ok(None)
            }
            _ => Err(invalid("Unknown spool record.")),
        }
    }
}

#[derive(Serialize)]
enum Record<'a> {
    Batch(BatchRef<'a>),
    End(u64),
}

struct BatchRef<'a> {
    rows: &'a [Row],
    columns: &'a [Column],
    context: &'a Context,
    memory: &'a Arc<Allowance>,
    cancel: &'a AtomicBool,
}

impl Serialize for BatchRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.rows.len()))?;
        for row in self.rows {
            check_cancelled(self.cancel).map_err(S::Error::custom)?;
            sequence.serialize_element(&RowRef { row, batch: self })?;
        }
        sequence.end()
    }
}

struct RowRef<'a, 'b> {
    row: &'a Row,
    batch: &'a BatchRef<'b>,
}

#[derive(Serialize, Deserialize)]
struct Cell<'a> {
    #[serde(borrow)]
    raw: Option<&'a str>,
    #[serde(borrow)]
    logical: Logical<'a>,
}

#[derive(Serialize, Deserialize)]
enum Logical<'a> {
    Null,
    Boolean(bool),
    Integer(i64),
    Double(f64),
    Decimal,
    Text,
    Bytes(#[serde(borrow)] &'a [u8]),
    Date(i32),
    Timestamp {
        value: i64,
        nanos: bool,
        instant: bool,
    },
    Nested,
}

impl Serialize for RowRef<'_, '_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut sequence = serializer.serialize_seq(Some(self.row.len()))?;
        for (column, raw) in self.batch.columns.iter().zip(self.row) {
            check_cancelled(self.batch.cancel).map_err(S::Error::custom)?;
            // Binary normalization is the only owned value. Reserve its worst
            // size before decoding; all other values borrow the source.
            let _normalization = self
                .batch
                .memory
                .acquire(raw.as_ref().map_or(0, String::len))
                .map_err(S::Error::custom)?;
            let value = value::normalize(Kind::of(&column.data_type), raw.as_deref())
                .map_err(S::Error::custom)?;
            let logical = match &value {
                Value::Null => Logical::Null,
                Value::Boolean(value) => Logical::Boolean(*value),
                Value::Integer(value) => Logical::Integer(*value),
                Value::Float(value) => Logical::Double(*value),
                Value::Decimal(_) => Logical::Decimal,
                Value::Bytes(value) => Logical::Bytes(value),
                Value::Nested(_) => Logical::Nested,
                Value::Text(text) if self.batch.context.iso_dates() => {
                    if column.data_type.eq_ignore_ascii_case("date") {
                        temporal::date(text).map_or(Logical::Text, Logical::Date)
                    } else {
                        temporal::Timestamp::from_type(&column.data_type)
                            .ok()
                            .flatten()
                            .and_then(|timestamp| {
                                timestamp.parse(text).ok().map(|value| Logical::Timestamp {
                                    value,
                                    nanos: timestamp.nanos(),
                                    instant: timestamp.instant,
                                })
                            })
                            .unwrap_or(Logical::Text)
                    }
                }
                Value::Text(_) => Logical::Text,
            };
            sequence.serialize_element(&Cell {
                raw: raw.as_deref(),
                logical,
            })?;
        }
        sequence.end()
    }
}

struct Counter(usize);
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|length| *length <= budget::MAX_RECORD_BYTES)
            .ok_or_else(|| invalid("Spool record exceeds 32 MiB."))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn count(value: &impl Serialize) -> io::Result<usize> {
    postcard::to_io(value, Counter(0))
        .map(|counter| counter.0)
        .map_err(codec)
}
fn codec(error: postcard::Error) -> io::Error {
    invalid(&format!("Invalid spool data: {error}"))
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn ensure_downloading(status: &Status) -> io::Result<()> {
    match status {
        Status::Downloading => Ok(()),
        Status::Cancelled => Err(io::Error::other(super::Cancelled)),
        Status::Failed(message) => Err(io::Error::other(message.clone())),
        Status::Complete { .. } => Err(invalid("Spool download is already complete.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, thread, time::Duration};

    fn new_spool(columns: &[Column], context: &Context) -> io::Result<(Arc<Spool>, Producer)> {
        Spool::new_in(columns, context, budget::Budget::new(1024 * budget::MIB))
    }

    fn schema() -> Vec<Column> {
        vec![Column {
            name: "value".into(),
            data_type: "STRING".into(),
        }]
    }

    #[test]
    fn independent_readers_wait_for_commits_and_validate_the_end() {
        let (spool, mut producer) = new_spool(&schema(), &Context::default()).unwrap();
        let mut first = spool.reader().unwrap();
        let (sender, receiver) = mpsc::channel();
        let waiting = thread::spawn(move || {
            let row = first.next(&AtomicBool::new(false)).unwrap().unwrap();
            sender.send(row.rows()[0][0].clone()).unwrap();
            assert!(first.next(&AtomicBool::new(false)).unwrap().is_none());
        });
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        producer
            .append(
                &[vec![Some("héllo\nworld".into())]],
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .as_deref(),
            Some("héllo\nworld")
        );
        producer.finish(&AtomicBool::new(false)).unwrap();
        waiting.join().unwrap();
        let mut replay = spool.reader().unwrap();
        assert_eq!(
            replay
                .next(&AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .rows()[0][0]
                .as_deref(),
            Some("héllo\nworld")
        );
        assert!(replay.next(&AtomicBool::new(false)).unwrap().is_none());
        spool.cancel();
        assert_eq!(spool.status(), Status::Complete { rows: 1 });
    }

    #[test]
    fn every_terminal_state_wakes_a_waiting_reader() {
        for cancelled in [false, true] {
            let (spool, producer) = new_spool(&schema(), &Context::default()).unwrap();
            let mut reader = spool.reader().unwrap();
            let waiting = thread::spawn(move || {
                reader
                    .next(&AtomicBool::new(false))
                    .err()
                    .unwrap()
                    .to_string()
            });
            if cancelled {
                spool.cancel();
            } else {
                spool.fail("injected producer failure");
            }
            let error = waiting.join().unwrap();
            assert!(error.contains(if cancelled {
                "cancelled"
            } else {
                "injected producer failure"
            }));
            drop(producer);
        }
    }

    #[test]
    fn incomplete_or_corrupt_records_never_complete() {
        let (spool, mut producer) = new_spool(&schema(), &Context::default()).unwrap();
        producer
            .append(&[vec![None]], &AtomicBool::new(false))
            .unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        let offset = spool.first_record;
        spool
            .file
            .write_all_at(&u32::MAX.to_le_bytes(), offset)
            .unwrap();
        assert!(
            spool
                .reader()
                .unwrap()
                .next(&AtomicBool::new(false))
                .err()
                .unwrap()
                .to_string()
                .contains("length")
        );
        spool.file.write_all_at(b"BADVERS!", 0).unwrap();
        assert!(spool.reader().is_err());
        let (spool, producer) = new_spool(&schema(), &Context::default()).unwrap();
        drop(producer);
        assert!(matches!(spool.status(), Status::Failed(_)));
    }

    #[test]
    fn typed_batches_preserve_every_connectors_server_text() {
        let columns: Vec<_> = [
            "boolean",
            "int8",
            "numeric",
            "varbinary",
            "binary",
            "bytea",
            "date",
            "timestamp(9)",
            "json",
        ]
        .into_iter()
        .map(|kind| Column {
            name: kind.into(),
            data_type: kind.into(),
        })
        .collect();
        let values = [
            "t",
            "9223372036854775807",
            "123456789012345678901234567890.12345678",
            "AFz/",
            "0x005cff",
            "\\x005cff",
            "1969-12-31",
            "2026-10-09 01:02:03.123456789",
            "[1,\n2]",
        ];
        let row: Row = values.into_iter().map(|value| Some(value.into())).collect();
        let (spool, mut producer) = new_spool(&columns, &Context::default()).unwrap();
        producer
            .append(std::slice::from_ref(&row), &AtomicBool::new(false))
            .unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        let mut reader = spool.reader().unwrap();
        assert_eq!(
            reader
                .next(&AtomicBool::new(false))
                .unwrap()
                .unwrap()
                .rows()[0],
            row
        );
        assert!(reader.next(&AtomicBool::new(false)).unwrap().is_none());
    }

    #[test]
    fn empty_results_and_cancelled_finalization_have_distinct_outcomes() {
        let (spool, producer) = new_spool(&schema(), &Context::default()).unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        assert!(
            spool
                .reader()
                .unwrap()
                .next(&AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
        assert_eq!(spool.status(), Status::Complete { rows: 0 });
        let (spool, mut producer) = new_spool(&schema(), &Context::default()).unwrap();
        producer
            .append(&[vec![Some("retained".into())]], &AtomicBool::new(false))
            .unwrap();
        spool.cancel();
        assert!(producer.finish(&AtomicBool::new(false)).is_err());
        assert_eq!(spool.status(), Status::Cancelled);
        assert!(
            spool
                .reader()
                .unwrap()
                .next(&AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn a_wrong_end_count_and_a_hostile_batch_count_stop_before_publication() {
        let (spool, mut producer) = new_spool(&schema(), &Context::default()).unwrap();
        producer
            .append(&[vec![Some("one".into())]], &AtomicBool::new(false))
            .unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        let end = spool.bytes();
        spool.file.write_all_at(&[1, 2], end - 2).unwrap();
        let mut reader = spool.reader().unwrap();
        assert!(reader.next(&AtomicBool::new(false)).unwrap().is_some());
        assert!(reader.next(&AtomicBool::new(false)).is_err());
        // Tag 0, then a valid varint claiming far too many rows. It must be
        // rejected before a Vec reserves the declared capacity.
        let payload = postcard::to_stdvec(&(0u32, u64::MAX)).unwrap();
        spool
            .file
            .write_all_at(&(payload.len() as u32).to_le_bytes(), spool.first_record)
            .unwrap();
        spool
            .file
            .write_all_at(&payload, spool.first_record + 4)
            .unwrap();
        assert!(
            spool
                .reader()
                .unwrap()
                .next(&AtomicBool::new(false))
                .err()
                .unwrap()
                .to_string()
                .contains("row count")
        );
    }

    #[test]
    fn cloned_rows_keep_their_charge_after_the_reader_and_batch_drop() {
        let budget = budget::Budget::new(512 * budget::MIB);
        let (spool, mut producer) =
            Spool::new_in(&schema(), &Context::default(), budget.clone()).unwrap();
        producer
            .append(&[vec![Some("owned".into())]], &AtomicBool::new(false))
            .unwrap();
        producer.finish(&AtomicBool::new(false)).unwrap();
        let mut reader = spool.reader().unwrap();
        let batch = reader.next(&AtomicBool::new(false)).unwrap().unwrap();
        let rows = batch.rows().clone();
        drop(batch);
        drop(reader);
        assert!(budget.acquire(353 * budget::MIB).is_err());
        assert_eq!(rows[0][0].as_deref(), Some("owned"));
        drop(rows);
        assert!(budget.acquire(353 * budget::MIB).is_ok());
    }

    #[test]
    fn normalization_failure_is_contextual_and_published_without_dropping_the_producer() {
        let columns = vec![Column {
            name: "enabled".into(),
            data_type: "boolean".into(),
        }];
        let (spool, mut producer) = new_spool(&columns, &Context::default()).unwrap();
        producer
            .append(&[vec![Some("true".into())]], &AtomicBool::new(false))
            .unwrap();
        let mut reader = spool.reader().unwrap();
        let waiting = thread::spawn(move || {
            loop {
                match reader.next(&AtomicBool::new(false)) {
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("A failed download completed"),
                    Err(error) => return error.to_string(),
                }
            }
        });
        // The helper below observes the sticky terminal directly instead of
        // relying on dropping the live producer to wake the reader.
        let error = producer
            .append(&[vec![Some("not-bool".into())]], &AtomicBool::new(false))
            .unwrap_err()
            .to_string();
        for expected in ["enabled", "row 2", "not-bool", "Invalid value"] {
            assert!(error.contains(expected), "{error}");
        }
        assert_eq!(spool.status(), Status::Failed(error.clone()));
        assert_eq!(waiting.join().unwrap(), error);
        assert!(producer.finish(&AtomicBool::new(false)).is_err());
        assert_eq!(spool.status(), Status::Failed(error));
    }
}
