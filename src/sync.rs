//! Locks that stay usable after a panic.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Locks a mutex also when a thread panicked while it held the lock. A
/// panic in one worker thread then does not make each later lock of the
/// same state panic too, for example in the window thread.
pub(crate) trait LockExt<T: ?Sized> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
}

impl<T: ?Sized> LockExt<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::LockExt as _;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_lock_stays_usable_after_a_panic_in_its_holder() {
        let value = Arc::new(Mutex::new(1));
        let holder = value.clone();
        let _ = std::thread::spawn(move || {
            let _guard = holder.lock().unwrap();
            panic!("a synthetic panic while the lock is held");
        })
        .join();
        assert!(value.is_poisoned());
        *value.lock_unpoisoned() += 1;
        assert_eq!(*value.lock_unpoisoned(), 2);
    }
}
