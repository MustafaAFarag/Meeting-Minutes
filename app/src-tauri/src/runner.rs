use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::queue::JobQueue;

/// What the widget is doing right now. One recording at a time; writing the notes for finished
/// recordings runs on its own, in `queue`.
#[derive(Default)]
pub struct RunState {
    active: Arc<AtomicBool>,
    recording: AtomicBool,
    /// Passed to `mn_core::record`; setting it stops the recording.
    pub interrupt: Arc<AtomicBool>,
    pub queue: JobQueue,
}

/// Held for the whole recording; dropping it frees the slot for the next `start`.
pub struct RunGuard {
    active: Arc<AtomicBool>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

impl RunState {
    /// Claims the recording slot. Errors if a recording is already active. Processing does not
    /// hold this slot.
    pub fn begin(&self) -> Result<RunGuard, String> {
        self.active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Already recording".to_string())?;
        self.interrupt.store(false, Ordering::SeqCst);
        Ok(RunGuard {
            active: Arc::clone(&self.active),
        })
    }

    #[cfg(test)]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }

    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::SeqCst);
    }

    /// A recording owns the slot, including the moment it is opening or being saved.
    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::SeqCst) || self.active.load(Ordering::SeqCst)
    }

    /// Asks a running recording to stop.
    pub fn request_stop(&self) -> Result<(), String> {
        if !self.recording.load(Ordering::SeqCst) {
            return Err("Not recording".to_string());
        }
        self.interrupt.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// For quitting: stops a recording and waits (up to `timeout`) for the WAV files to be closed.
    pub fn stop_recording_and_wait(&self, timeout: Duration) {
        if self.request_stop().is_err() {
            return;
        }
        let deadline = Instant::now() + timeout;
        while self.recording.load(Ordering::SeqCst) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Meetings usually take about 30 minutes. A recording that runs longer gets a reminder at 30
/// minutes and again every 30 minutes after that. It never stops by itself.
pub const REMIND_EVERY_SECS: u64 = 30 * 60;

/// The reminder interval. `MN_REMIND_SECS` shortens it for testing.
pub fn remind_every() -> u64 {
    std::env::var("MN_REMIND_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|&secs| secs > 0)
        .unwrap_or(REMIND_EVERY_SECS)
}

/// The number of the reminder that is due at `elapsed` seconds (1 for the first), if it is newer
/// than `sent`, the last one already shown.
pub fn reminder_due(elapsed: u64, sent: u64, every: u64) -> Option<u64> {
    let due = elapsed / every;
    (due > sent).then_some(due)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_come_once_per_interval() {
        let every = REMIND_EVERY_SECS;
        assert_eq!(reminder_due(0, 0, every), None);
        assert_eq!(reminder_due(every - 1, 0, every), None);
        assert_eq!(reminder_due(every, 0, every), Some(1));
        assert_eq!(reminder_due(every + 5, 1, every), None);
        assert_eq!(reminder_due(2 * every, 1, every), Some(2));
        // A gap in the status events skips ahead instead of firing the missed ones one by one.
        assert_eq!(reminder_due(3 * every + 1, 1, every), Some(3));
    }

    #[test]
    fn second_begin_is_refused_while_the_first_is_held() {
        let state = RunState::default();
        let guard = state.begin().unwrap();
        assert!(state.is_active());
        assert_eq!(state.begin().err().as_deref(), Some("Already recording"));
        drop(guard);
        assert!(!state.is_active());
        assert!(state.begin().is_ok());
    }

    #[test]
    fn begin_clears_a_stale_interrupt() {
        let state = RunState::default();
        state.interrupt.store(true, Ordering::SeqCst);
        let _guard = state.begin().unwrap();
        assert!(!state.interrupt.load(Ordering::SeqCst));
    }

    #[test]
    fn a_held_slot_or_the_recording_flag_blocks_an_update() {
        let state = RunState::default();
        assert!(!state.is_recording());
        let guard = state.begin().unwrap();
        assert!(state.is_recording());
        drop(guard);
        assert!(!state.is_recording());
        state.set_recording(true);
        assert!(state.is_recording());
        state.set_recording(false);
        assert!(!state.is_recording());
    }

    #[test]
    fn stop_only_works_while_recording() {
        let state = RunState::default();
        let _guard = state.begin().unwrap();
        assert!(state.request_stop().is_err());
        assert!(!state.interrupt.load(Ordering::SeqCst));
        state.set_recording(true);
        assert!(state.request_stop().is_ok());
        assert!(state.interrupt.load(Ordering::SeqCst));
    }
}
