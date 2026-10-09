use crate::model::Row;
use std::{
    collections::HashMap,
    io,
    ops::Index,
    sync::{Arc, Mutex},
};

/// Preview rows in immutable batches. Cloning this store shares cells, and
/// appending a batch never copies a batch held by an export.
#[derive(Clone, Default)]
pub struct Rows {
    batches: Vec<Arc<Batch>>,
    len: usize,
}

struct Batch {
    start: usize,
    rows: Vec<Row>,
    bytes: usize,
}

impl Rows {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn extend(&mut self, rows: Vec<Row>) {
        if rows.is_empty() {
            return;
        }
        let bytes = rows.capacity() * std::mem::size_of::<Row>()
            + rows
                .iter()
                .map(|row| {
                    row.capacity() * std::mem::size_of::<Option<String>>()
                        + row.iter().flatten().map(String::capacity).sum::<usize>()
                })
                .sum::<usize>();
        let start = self.len;
        self.len += rows.len();
        self.batches.push(Arc::new(Batch { start, rows, bytes }));
    }

    pub fn get(&self, row: usize) -> Option<&Row> {
        let batch = self
            .batches
            .partition_point(|batch| batch.start <= row)
            .checked_sub(1)?;
        let batch = &self.batches[batch];
        batch.rows.get(row - batch.start)
    }

    pub(super) fn retain_for_export(&self) -> io::Result<Lease> {
        Lease::acquire(self, &EXPORT_BUDGET, MAX_SOURCE_BYTES)
    }
}

impl From<Vec<Row>> for Rows {
    fn from(rows: Vec<Row>) -> Self {
        let mut store = Self::default();
        store.extend(rows);
        store
    }
}

impl FromIterator<Row> for Rows {
    fn from_iter<T: IntoIterator<Item = Row>>(iter: T) -> Self {
        Vec::from_iter(iter).into()
    }
}

impl Index<usize> for Rows {
    type Output = Row;
    fn index(&self, row: usize) -> &Self::Output {
        self.get(row).expect("result row index")
    }
}

const MAX_SOURCE_BYTES: usize = 512 * 1024 * 1024;
static EXPORT_BUDGET: Mutex<Option<Budget>> = Mutex::new(None);

#[derive(Default)]
struct Budget {
    bytes: usize,
    batches: HashMap<usize, (usize, usize)>,
}

/// The batch references keep allocation identities valid until accounting
/// ends. Different snapshots of the same batch count its bytes only once.
pub(super) struct Lease {
    batches: Vec<Arc<Batch>>,
    budget: &'static Mutex<Option<Budget>>,
}

impl Lease {
    fn acquire(
        rows: &Rows,
        budget: &'static Mutex<Option<Budget>>,
        limit: usize,
    ) -> io::Result<Self> {
        let mut lock = budget.lock().unwrap();
        let state = lock.get_or_insert_with(Budget::default);
        let additional: usize = rows
            .batches
            .iter()
            .filter(|batch| !state.batches.contains_key(&(Arc::as_ptr(batch) as usize)))
            .map(|batch| batch.bytes)
            .sum();
        if additional > limit.saturating_sub(state.bytes) {
            return Err(io::Error::other(
                "Export memory limit reached. Close another export and try again.",
            ));
        }
        for batch in &rows.batches {
            let entry = state
                .batches
                .entry(Arc::as_ptr(batch) as usize)
                .or_insert((batch.bytes, 0));
            entry.1 += 1;
        }
        state.bytes += additional;
        Ok(Self {
            batches: rows.batches.clone(),
            budget,
        })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut lock = self.budget.lock().unwrap();
        let state = lock.as_mut().unwrap();
        for batch in &self.batches {
            let id = Arc::as_ptr(batch) as usize;
            let entry = state.batches.get_mut(&id).unwrap();
            entry.1 -= 1;
            if entry.1 == 0 {
                let (bytes, _) = state.batches.remove(&id).unwrap();
                state.bytes -= bytes;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_share_batches_and_do_not_change_after_append() {
        let mut rows = Rows::from(vec![vec![Some("first".into())]]);
        let snapshot = rows.clone();
        rows.extend(vec![vec![Some("second".into())]]);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(rows.len(), 2);
        assert!(std::ptr::eq(&snapshot[0], &rows[0]));
        assert_eq!(rows[1][0].as_deref(), Some("second"));
        assert!(rows.get(2).is_none());
    }

    #[test]
    fn a_shared_batch_counts_once_and_a_failed_lease_reserves_nothing() {
        let budget: &'static Mutex<Option<Budget>> = Box::leak(Box::new(Mutex::new(None)));
        let rows = Rows::from(vec![vec![Some("value".into())]]);
        let bytes = rows.batches[0].bytes;
        let first = Lease::acquire(&rows, budget, bytes).unwrap();
        let second = Lease::acquire(&rows, budget, bytes).unwrap();
        let other = Rows::from(vec![vec![Some("other".into())]]);
        assert!(Lease::acquire(&other, budget, bytes).is_err());
        drop(first);
        assert_eq!(budget.lock().unwrap().as_ref().unwrap().bytes, bytes);
        drop(second);
        assert_eq!(budget.lock().unwrap().as_ref().unwrap().bytes, 0);
        assert!(Lease::acquire(&other, budget, bytes * 2).is_ok());
    }
}
