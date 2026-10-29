//! Returns free malloc pages to macOS after the windows stop drawing.
//!
//! The macOS allocator keeps the pages of freed blocks for reuse. After work
//! that allocated and freed much memory, for example long query results or a
//! streamed reply, those pages stay in the process footprint until memory
//! gets low. When no display link has run for [`RELIEF_DELAY`], a background
//! thread asks all malloc zones to return their free pages, one time for each
//! idle period. The call takes milliseconds and does not change live memory.

use std::{
    ffi::c_void,
    sync::{OnceLock, mpsc},
    thread,
    time::{Duration, Instant},
};

/// How long all display links must stay stopped before the relief.
pub(crate) const RELIEF_DELAY: Duration = Duration::from_secs(5);

unsafe extern "C" {
    /// Returns free pages of `zone`, or of all zones when it is null, until
    /// `goal` bytes are free. A goal of 0 returns as many as possible.
    fn malloc_zone_pressure_relief(zone: *mut c_void, goal: usize) -> usize;
}

static DRAWING: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

/// Records whether a display link runs. Called when the first display link
/// starts and when the last one stops.
pub(crate) fn set_drawing(drawing: bool) {
    let sender = DRAWING.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("memory relief".into())
            .spawn(move || run(receiver))
            .expect("could not start the memory relief thread");
        sender
    });
    let _ = sender.send(drawing);
}

fn run(receiver: mpsc::Receiver<bool>) {
    let mut timer = ReliefTimer::default();
    loop {
        let message = match timer.wait(Instant::now()) {
            Some(wait) => receiver.recv_timeout(wait),
            None => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match message {
            Ok(drawing) => timer.set_drawing(drawing, Instant::now()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if timer.take_due(Instant::now()) {
            // SAFETY: the function takes no pointers from Rust except the null
            // zone, which selects all zones.
            unsafe { malloc_zone_pressure_relief(std::ptr::null_mut(), 0) };
        }
    }
}

/// Decides when the relief runs: [`RELIEF_DELAY`] after the display links
/// stop, and not again before they start and stop again.
#[derive(Default)]
struct ReliefTimer {
    idle_since: Option<Instant>,
}

impl ReliefTimer {
    fn set_drawing(&mut self, drawing: bool, now: Instant) {
        self.idle_since = (!drawing).then_some(now);
    }

    /// How long to wait for the relief, or `None` when none is pending.
    fn wait(&self, now: Instant) -> Option<Duration> {
        self.idle_since
            .map(|since| RELIEF_DELAY.saturating_sub(now.duration_since(since)))
    }

    /// Whether the relief is due now. A due relief is not due again until
    /// the display links start and stop again.
    fn take_due(&mut self, now: Instant) -> bool {
        let due = self.wait(now) == Some(Duration::ZERO);
        if due {
            self.idle_since = None;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::{RELIEF_DELAY, ReliefTimer};
    use std::time::{Duration, Instant};

    #[test]
    fn the_relief_runs_once_after_the_delay() {
        let start = Instant::now();
        let mut timer = ReliefTimer::default();
        assert_eq!(timer.wait(start), None);

        timer.set_drawing(false, start);
        assert_eq!(timer.wait(start), Some(RELIEF_DELAY));
        assert!(!timer.take_due(start + RELIEF_DELAY - Duration::from_millis(1)));
        assert!(timer.take_due(start + RELIEF_DELAY));
        assert_eq!(timer.wait(start + RELIEF_DELAY * 2), None);
        assert!(!timer.take_due(start + RELIEF_DELAY * 2));
    }

    #[test]
    fn drawing_before_the_delay_cancels_the_relief() {
        let start = Instant::now();
        let mut timer = ReliefTimer::default();
        timer.set_drawing(false, start);
        timer.set_drawing(true, start + RELIEF_DELAY / 2);
        assert_eq!(timer.wait(start + RELIEF_DELAY), None);
        assert!(!timer.take_due(start + RELIEF_DELAY));

        // The next idle period starts its own delay.
        let idle = start + RELIEF_DELAY * 3;
        timer.set_drawing(false, idle);
        assert!(!timer.take_due(start + RELIEF_DELAY * 4 - Duration::from_millis(1)));
        assert!(timer.take_due(idle + RELIEF_DELAY));
    }
}
