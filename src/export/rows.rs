use crate::model::Row;
use std::{io, ops::Index, sync::Arc};

/// Preview rows in immutable batches. Cloning this store shares cells, and
/// appending a batch never copies a batch held by an export.
#[derive(Clone, Default)]
pub struct Rows {
    batches: Vec<Chunk>,
    len: usize,
    context: Arc<super::Context>,
}

#[derive(Clone)]
struct Chunk {
    start: usize,
    batch: Arc<Batch>,
}

struct Batch {
    rows: Vec<Row>,
    bytes: usize,
    _allocation: Option<super::budget::Allocation>,
}

impl Rows {
    pub(super) fn batches(&self) -> impl Iterator<Item = &[Row]> {
        self.batches.iter().map(|chunk| chunk.batch.rows.as_slice())
    }

    pub fn context(&self) -> &super::Context {
        &self.context
    }

    pub fn set_context(&mut self, context: super::Context) {
        self.context = Arc::new(context);
    }
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
        self.batches.push(Chunk {
            start,
            batch: Arc::new(Batch {
                rows,
                bytes,
                _allocation: None,
            }),
        });
    }

    pub(crate) fn accounted(
        rows: Vec<Row>,
        allocation: super::budget::Allocation,
        context: super::Context,
    ) -> Self {
        let mut store = Self::from(rows);
        if let Some(batch) = store.batches.first_mut() {
            Arc::get_mut(&mut batch.batch).unwrap()._allocation = Some(allocation);
        }
        store.set_context(context);
        store
    }

    pub(crate) fn append_shared(&mut self, other: &Self) {
        if self.is_empty() {
            self.context = other.context.clone();
        }
        let start = self.len;
        self.batches.extend(other.batches.iter().map(|chunk| Chunk {
            start: start + chunk.start,
            batch: chunk.batch.clone(),
        }));
        self.len += other.len;
    }

    pub(super) fn retained_bytes(&self) -> usize {
        self.batches.iter().map(|chunk| chunk.batch.bytes).sum()
    }

    pub(super) fn accounted_bytes(&self) -> usize {
        self.batches
            .iter()
            .map(|chunk| {
                chunk
                    .batch
                    ._allocation
                    .as_ref()
                    .map_or(0, super::budget::Allocation::bytes)
            })
            .sum()
    }

    pub fn get(&self, row: usize) -> Option<&Row> {
        let batch = self
            .batches
            .partition_point(|batch| batch.start <= row)
            .checked_sub(1)?;
        let batch = &self.batches[batch];
        batch.batch.rows.get(row - batch.start)
    }

    pub(super) fn metadata_bytes(&self) -> usize {
        self.batches.len() * std::mem::size_of::<Chunk>()
    }

    pub(super) fn retain_for_export(&self) -> io::Result<Lease> {
        Lease::acquire(self, super::budget::GLOBAL.clone())
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

/// Keep allocation identities alive until the shared accounting guard drops.
pub(super) struct Lease {
    _shared: super::budget::Shared,
    _batches: Vec<Chunk>,
    _metadata: super::budget::Allocation,
}

impl Lease {
    fn acquire(rows: &Rows, budget: Arc<super::budget::Budget>) -> io::Result<Self> {
        let metadata = budget.acquire(rows.metadata_bytes())?;
        let scratch = budget.acquire(rows.batches.len() * std::mem::size_of::<(usize, usize)>())?;
        let batches: Vec<_> = rows
            .batches
            .iter()
            .filter(|batch| batch.batch._allocation.is_none())
            .map(|batch| (Arc::as_ptr(&batch.batch) as usize, batch.batch.bytes))
            .collect();
        let shared = budget.share(&batches)?;
        drop(batches);
        drop(scratch);
        Ok(Self {
            _shared: shared,
            _batches: rows.batches.clone(),
            _metadata: metadata,
        })
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
    fn combining_shared_batches_keeps_indices_and_allocation_identity() {
        let first = Rows::from(vec![
            vec![Some("first".into())],
            vec![Some("second".into())],
        ]);
        let second = Rows::from(vec![vec![Some("third".into())]]);
        let mut combined = Rows::default();
        combined.append_shared(&first);
        combined.append_shared(&second);
        assert_eq!(combined.len(), 3);
        assert!(std::ptr::eq(&combined[0], &first[0]));
        assert!(std::ptr::eq(&combined[2], &second[0]));
        drop(first);
        drop(second);
        assert_eq!(combined[1][0].as_deref(), Some("second"));
        assert_eq!(combined[2][0].as_deref(), Some("third"));
    }

    #[test]
    fn a_shared_batch_counts_once_and_a_failed_lease_reserves_nothing() {
        let rows = Rows::from(vec![vec![Some("value".into())]]);
        let bytes = rows.batches[0].batch.bytes;
        let budget = super::super::budget::Budget::new(bytes + 1300);
        let first = Lease::acquire(&rows, budget.clone()).unwrap();
        let second = Lease::acquire(&rows, budget.clone()).unwrap();
        let other = Rows::from(vec![vec![Some("other".into())]]);
        assert!(Lease::acquire(&other, budget.clone()).is_err());
        drop(first);
        assert!(Lease::acquire(&other, budget.clone()).is_err());
        drop(second);
        assert!(Lease::acquire(&other, budget).is_ok());
    }
}
