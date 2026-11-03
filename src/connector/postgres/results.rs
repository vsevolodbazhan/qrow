//! A growing preview prefix or a capacity-one direct-export channel.
use super::*;
use crate::export::budget;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Mode {
    Preview,
    Export,
}

#[derive(Clone)]
enum Failure {
    Query(String),
    Other(String),
}

impl Failure {
    fn of(error: &anyhow::Error) -> Self {
        if let Some(error) = error.downcast_ref::<QueryError>() {
            Self::Query(error.0.clone())
        } else {
            Self::Other(format!("{error:#}"))
        }
    }
    fn error(self) -> anyhow::Error {
        match self {
            Self::Query(message) => QueryError(message).into(),
            Self::Other(message) => anyhow::anyhow!(message),
        }
    }
}

#[derive(Default)]
struct State {
    rows: usize,
    bytes: u64,
    limited: bool,
    outcome: Option<std::result::Result<(), Failure>>,
}

#[derive(Default)]
pub(super) struct Progress {
    state: Mutex<State>,
    changed: Condvar,
}

impl Progress {
    pub(super) fn fail(&self, error: &anyhow::Error) {
        self.state.lock().unwrap().outcome = Some(Err(Failure::of(error)));
        self.changed.notify_all();
    }
    pub(super) fn finish(&self, result: &Result<()>) {
        self.state.lock().unwrap().outcome = Some(result.as_ref().copied().map_err(Failure::of));
        self.changed.notify_all();
    }
    pub(super) fn completed(&self) -> Result<bool> {
        match self.state.lock().unwrap().outcome.clone() {
            None => Ok(false),
            Some(Ok(())) => Ok(true),
            Some(Err(error)) => Err(error.error()),
        }
    }
    pub(super) fn limited(&self) -> bool {
        self.state.lock().unwrap().limited
    }
    pub(super) fn wait(&self) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(outcome) = &state.outcome {
                return outcome.clone().map_err(Failure::error);
            }
            state = self.changed.wait(state).unwrap();
        }
    }
    fn row(&self, consumed: usize) -> Result<Option<u64>> {
        let mut state = self.state.lock().unwrap();
        loop {
            if let Some(Err(error)) = &state.outcome {
                return Err(error.clone().error());
            }
            if consumed < state.rows {
                return Ok(Some(state.bytes));
            }
            if state.outcome.is_some() {
                return Ok(None);
            }
            state = self.changed.wait(state).unwrap();
        }
    }
}

pub(super) struct Results {
    pub(super) columns: Vec<Column>,
    pub(super) exhausted: bool,
    pub(super) context: crate::export::Context,
    pub(super) rows: Rows,
}

pub(super) enum Rows {
    Prefix {
        file: File,
        offset: u64,
        consumed: usize,
    },
    Direct {
        receiver: tokio::sync::mpsc::Receiver<Row>,
        pending: Option<Row>,
    },
}

pub(super) enum Output {
    Prefix(Prefix),
    Direct(tokio::sync::mpsc::Sender<Row>),
}

pub(super) struct Prefix {
    writer: BufWriter<File>,
    rows: usize,
    owned_bytes: usize,
    encoded: u64,
    published: u64,
    limited: bool,
}

pub(super) fn source(mode: Mode) -> Result<(Output, Rows)> {
    match mode {
        Mode::Preview => {
            let file = tempfile::tempfile()?;
            let reader = file.try_clone()?;
            Ok((
                Output::Prefix(Prefix {
                    writer: BufWriter::new(file),
                    rows: 0,
                    owned_bytes: 0,
                    encoded: 0,
                    published: 0,
                    limited: false,
                }),
                Rows::Prefix {
                    file: reader,
                    offset: 0,
                    consumed: 0,
                },
            ))
        }
        Mode::Export => {
            let (sender, receiver) = tokio::sync::mpsc::channel(1);
            Ok((
                Output::Direct(sender),
                Rows::Direct {
                    receiver,
                    pending: None,
                },
            ))
        }
    }
}

impl Prefix {
    pub(super) fn append(
        &mut self,
        row: &tokio_postgres::SimpleQueryRow,
        progress: &Progress,
    ) -> Result<()> {
        let bytes = row
            .len()
            .saturating_mul(std::mem::size_of::<Option<String>>())
            .saturating_add(
                (0..row.len())
                    .filter_map(|index| row.get(index))
                    .map(str::len)
                    .sum::<usize>(),
            );
        if self.rows >= MAX_RESULT_ROWS || self.owned_bytes.saturating_add(bytes) > MAX_RESULT_BYTES
        {
            self.limited = true;
            progress.state.lock().unwrap().limited = true;
        }
        if self.limited {
            return Ok(());
        }
        self.writer.write_all(&(row.len() as u64).to_le_bytes())?;
        self.encoded += 8;
        for index in 0..row.len() {
            match row.get(index) {
                None => self.writer.write_all(&u64::MAX.to_le_bytes())?,
                Some(value) => {
                    self.writer.write_all(&(value.len() as u64).to_le_bytes())?;
                    self.writer.write_all(value.as_bytes())?;
                    self.encoded += value.len() as u64;
                }
            }
            self.encoded += 8;
        }
        self.rows += 1;
        self.owned_bytes += bytes;
        if self.rows.is_multiple_of(128)
            || self.rows == crate::model::PREVIEW_ROWS
            || self.encoded - self.published >= 64 * 1024
        {
            self.commit(progress)?;
        }
        Ok(())
    }
    pub(super) fn commit(&mut self, progress: &Progress) -> Result<()> {
        self.writer.flush()?;
        self.published = self.encoded;
        let mut state = progress.state.lock().unwrap();
        state.rows = self.rows;
        state.bytes = self.encoded;
        state.limited = self.limited;
        drop(state);
        progress.changed.notify_all();
        Ok(())
    }
}

fn read(file: &File, offset: &mut u64, committed: u64, bytes: &mut [u8]) -> Result<()> {
    anyhow::ensure!(
        bytes.len() as u64 <= committed.saturating_sub(*offset),
        "Incomplete Postgres preview record"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(bytes, *offset)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut read = 0;
        while read < bytes.len() {
            let count = file.seek_read(&mut bytes[read..], *offset + read as u64)?;
            anyhow::ensure!(count > 0, "Incomplete Postgres preview record");
            read += count;
        }
    }
    *offset += bytes.len() as u64;
    Ok(())
}

impl Results {
    pub(super) fn fetch(
        &mut self,
        count: usize,
        progress: &Progress,
        runtime: &Runtime,
    ) -> Result<Batch> {
        let count = if matches!(self.rows, Rows::Direct { .. }) {
            count.min(1000)
        } else {
            count
        };
        let mut rows = Vec::with_capacity(count.min(1000));
        match &mut self.rows {
            Rows::Prefix {
                file,
                offset,
                consumed,
            } => {
                for _ in 0..count {
                    let Some(committed) = progress.row(*consumed)? else {
                        break;
                    };
                    let mut length = [0; 8];
                    read(file, offset, committed, &mut length)?;
                    let width = u64::from_le_bytes(length) as usize;
                    anyhow::ensure!(
                        width == self.columns.len(),
                        "Invalid Postgres preview row width"
                    );
                    let mut row = Vec::with_capacity(width);
                    for _ in 0..width {
                        read(file, offset, committed, &mut length)?;
                        let length = u64::from_le_bytes(length);
                        if length == u64::MAX {
                            row.push(None);
                        } else {
                            anyhow::ensure!(
                                length <= MAX_RESULT_BYTES as u64,
                                "Invalid Postgres preview cell length"
                            );
                            let mut bytes = vec![0; length as usize];
                            read(file, offset, committed, &mut bytes)?;
                            row.push(Some(String::from_utf8(bytes)?));
                        }
                    }
                    rows.push(row);
                    *consumed += 1;
                }
            }
            Rows::Direct { receiver, pending } => {
                progress.completed()?;
                let mut bytes = rows.capacity() * std::mem::size_of::<Row>();
                for _ in 0..count {
                    let next = pending.take().or_else(|| runtime.block_on(receiver.recv()));
                    let Some(row) = next else {
                        progress.wait()?;
                        break;
                    };
                    let size = budget::row_bytes(&row)?;
                    if !rows.is_empty() && bytes + size > 48 * budget::MIB {
                        *pending = Some(row);
                        break;
                    }
                    bytes += size;
                    rows.push(row);
                }
            }
        }
        self.exhausted = rows.is_empty();
        Ok(Batch { rows })
    }
}
