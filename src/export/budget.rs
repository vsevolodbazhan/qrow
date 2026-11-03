//! One counter for shared sources and live export allocations.
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, LazyLock, Mutex},
};

pub(crate) const MIB: usize = 1024 * 1024;
pub(crate) const MAX_CELL_BYTES: usize = 8 * MIB;
pub(crate) const MAX_ROW_BYTES: usize = 16 * MIB;
pub(crate) const MAX_RECORD_BYTES: usize = 32 * MIB;
pub(crate) static GLOBAL: LazyLock<Arc<Budget>> = LazyLock::new(|| Budget::new(1024 * MIB));

pub(crate) struct Budget {
    limit: usize,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    used: usize,
    shared: BTreeMap<usize, (usize, usize)>,
}

/// One export's working allowance cannot consume another stage's allowance.
pub(crate) struct Allowance {
    global: Arc<Budget>,
    limit: usize,
    used: Mutex<usize>,
}

pub(crate) struct Allocation {
    global: Arc<Budget>,
    allowance: Option<Arc<Allowance>>,
    bytes: usize,
}

pub(crate) struct Shared {
    global: Arc<Budget>,
    batches: Vec<(usize, usize)>,
    overhead: usize,
}

impl Budget {
    pub(crate) fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            state: Mutex::new(State::default()),
        })
    }

    pub(crate) fn allowance(self: &Arc<Self>, limit: usize) -> io::Result<Arc<Allowance>> {
        let mut state = self.state.lock().unwrap();
        if limit > self.limit.saturating_sub(state.used) {
            return Err(full());
        }
        // Reserve headroom at admission. A writer cannot borrow the producer's
        // reservation while the producer waits for its next server response.
        state.used += limit;
        Ok(Arc::new(Allowance {
            global: self.clone(),
            limit,
            used: Mutex::new(0),
        }))
    }

    pub(crate) fn acquire(self: &Arc<Self>, bytes: usize) -> io::Result<Allocation> {
        let mut state = self.state.lock().unwrap();
        if bytes > self.limit.saturating_sub(state.used) {
            return Err(full());
        }
        state.used += bytes;
        Ok(Allocation {
            global: self.clone(),
            allowance: None,
            bytes,
        })
    }

    /// The owner must retain each allocation's Arc until this guard drops.
    pub(crate) fn share(self: &Arc<Self>, batches: &[(usize, usize)]) -> io::Result<Shared> {
        let mut state = self.state.lock().unwrap();
        let overhead = batches
            .len()
            .checked_mul(std::mem::size_of::<(usize, usize)>())
            .and_then(|bytes| bytes.checked_add(64))
            .ok_or_else(full)?;
        if overhead > self.limit.saturating_sub(state.used) {
            return Err(full());
        }
        let mut batches = batches.to_vec();
        batches.sort_unstable_by_key(|(id, _)| *id);
        if batches
            .windows(2)
            .any(|pair| pair[0].0 == pair[1].0 && pair[0].1 != pair[1].1)
        {
            return Err(io::Error::other("A shared allocation changed size."));
        }
        batches.dedup_by_key(|(id, _)| *id);
        let mut additional = overhead;
        for &(id, bytes) in &batches {
            if let Some((previous, _)) = state.shared.get(&id) {
                if *previous != bytes {
                    return Err(io::Error::other("A shared allocation changed size."));
                }
            } else {
                additional = additional
                    .checked_add(bytes)
                    .and_then(|sum| sum.checked_add(1024))
                    .ok_or_else(full)?;
            }
        }
        if additional > self.limit.saturating_sub(state.used) {
            return Err(full());
        }
        for &(id, bytes) in &batches {
            let entry = state.shared.entry(id).or_insert((bytes, 0));
            entry.1 += 1;
        }
        state.used += additional;
        Ok(Shared {
            global: self.clone(),
            batches,
            overhead,
        })
    }
}

impl Allowance {
    pub(crate) fn capacity(&self) -> usize {
        self.limit
    }
    pub(crate) fn acquire(self: &Arc<Self>, bytes: usize) -> io::Result<Allocation> {
        let mut used = self.used.lock().unwrap();
        if bytes > self.limit.saturating_sub(*used) {
            return Err(io::Error::other(
                "This export exceeds its working memory limit.",
            ));
        }
        *used += bytes;
        Ok(Allocation {
            global: self.global.clone(),
            allowance: Some(self.clone()),
            bytes,
        })
    }
}

impl Drop for Allocation {
    fn drop(&mut self) {
        if let Some(allowance) = &self.allowance {
            *allowance.used.lock().unwrap() -= self.bytes;
        } else {
            self.global.state.lock().unwrap().used -= self.bytes;
        }
    }
}

impl Allocation {
    pub(super) fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for Allowance {
    fn drop(&mut self) {
        self.global.state.lock().unwrap().used -= self.limit;
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        let mut state = self.global.state.lock().unwrap();
        for (id, _) in &self.batches {
            let entry = state.shared.get_mut(id).unwrap();
            entry.1 -= 1;
            if entry.1 == 0 {
                let (bytes, _) = state.shared.remove(id).unwrap();
                // Conservative ledger-node accounting includes tree slack.
                state.used -= bytes + 1024;
            }
        }
        state.used -= self.overhead;
    }
}

fn full() -> io::Error {
    io::Error::other("Export memory limit reached. Close another export and try again.")
}

pub(crate) fn row_bytes(row: &crate::model::Row) -> io::Result<usize> {
    let mut bytes = row
        .capacity()
        .checked_mul(std::mem::size_of::<Option<String>>())
        .ok_or_else(full)?;
    for text in row.iter().flatten() {
        if text.len() > MAX_CELL_BYTES {
            return Err(io::Error::other(
                "A result cell exceeds the 8 MiB export limit.",
            ));
        }
        bytes = bytes.checked_add(text.capacity()).ok_or_else(full)?;
    }
    if bytes > MAX_ROW_BYTES {
        return Err(io::Error::other(
            "A result row exceeds the 16 MiB export limit.",
        ));
    }
    Ok(bytes)
}

pub(crate) fn selected_row_bytes<'a>(
    values: impl Iterator<Item = Option<&'a str>>,
) -> io::Result<usize> {
    let mut bytes = 0usize;
    for value in values {
        if value.is_some_and(|text| text.len() > MAX_CELL_BYTES) {
            return Err(io::Error::other(
                "A result cell exceeds the 8 MiB export limit.",
            ));
        }
        bytes = bytes
            .checked_add(std::mem::size_of::<Option<String>>() + value.map_or(0, str::len))
            .ok_or_else(full)?;
    }
    if bytes > MAX_ROW_BYTES {
        return Err(io::Error::other(
            "A result row exceeds the 16 MiB export limit.",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_sources_and_working_allocations_use_one_counter() {
        let budget = Budget::new(2000);
        let first = budget.share(&[(1, 60), (1, 60)]).unwrap();
        let second = budget.share(&[(1, 60)]).unwrap();
        let writer = budget.allowance(30).unwrap();
        let allocation = writer.acquire(30).unwrap();
        assert!(writer.acquire(1).is_err());
        assert!(budget.acquire(800).is_err());
        drop(first);
        assert!(budget.acquire(900).is_err());
        drop(second);
        let remaining = budget.acquire(1970).unwrap();
        assert!(budget.acquire(1).is_err());
        drop(allocation);
        drop(remaining);
        drop(writer);
        assert_eq!(budget.state.lock().unwrap().used, 0);
    }

    #[test]
    fn writer_and_producer_have_independent_allowances() {
        let budget = Budget::new(100);
        let writer = budget.allowance(60).unwrap();
        let producer = budget.allowance(40).unwrap();
        // Even an empty writer cannot take the producer's admitted headroom.
        assert!(budget.allowance(1).is_err());
        let writing = writer.acquire(60).unwrap();
        assert!(writer.acquire(1).is_err());
        let fetching = producer.acquire(40).unwrap();
        assert!(budget.acquire(1).is_err());
        drop(writing);
        drop(fetching);
        drop(writer);
        drop(producer);
        assert_eq!(budget.state.lock().unwrap().used, 0);
    }

    #[test]
    fn two_slow_writers_cannot_take_an_admitted_producers_headroom() {
        use std::{sync::Barrier, thread};
        let budget = Budget::new(2800);
        let source = budget.share(&[(1, 60)]).unwrap();
        let producer = budget.allowance(1000).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let mut writers = Vec::new();
        for _ in 0..2 {
            let writer = budget.allowance(300).unwrap();
            let barrier = barrier.clone();
            writers.push(thread::spawn(move || {
                let _encoding = writer.acquire(300).unwrap();
                barrier.wait();
                barrier.wait();
            }));
        }
        barrier.wait();
        assert!(budget.allowance(45).is_err());
        let fetched = producer.acquire(1000).unwrap();
        barrier.wait();
        for writer in writers {
            writer.join().unwrap();
        }
        drop(fetched);
        drop(producer);
        drop(source);
        assert_eq!(budget.state.lock().unwrap().used, 0);
    }

    #[test]
    fn an_oversized_cell_or_row_stops_before_a_copy() {
        let row = vec![Some("x".repeat(MAX_CELL_BYTES + 1))];
        assert!(row_bytes(&row).unwrap_err().to_string().contains("cell"));
        let row = vec![
            Some("x".repeat(MAX_CELL_BYTES)),
            Some("x".repeat(MAX_CELL_BYTES)),
        ];
        assert!(row_bytes(&row).unwrap_err().to_string().contains("row"));
    }
}
