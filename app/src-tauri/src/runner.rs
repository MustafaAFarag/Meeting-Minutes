use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// What the widget is doing right now. One run (record + process, or a retry) at a time.
#[derive(Default)]
pub struct RunState {
    active: Arc<AtomicBool>,
    recording: AtomicBool,
    /// Passed to `mn_core::record`; setting it stops the recording.
    pub interrupt: Arc<AtomicBool>,
    last_session: Mutex<Option<PathBuf>>,
}

/// Held for the whole run; dropping it frees the slot for the next `start`.
pub struct RunGuard {
    active: Arc<AtomicBool>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.active.store(false, Ordering::SeqCst);
    }
}

impl RunState {
    /// Claims the single run slot. Errors if a run is already active.
    pub fn begin(&self) -> Result<RunGuard, String> {
        self.active
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "Already running".to_string())?;
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

    pub fn last_session(&self) -> Option<PathBuf> {
        self.last_session.lock().ok().and_then(|s| s.clone())
    }

    pub fn set_last_session(&self, dir: Option<PathBuf>) {
        if let Ok(mut s) = self.last_session.lock() {
            *s = dir;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_begin_is_refused_while_the_first_is_held() {
        let state = RunState::default();
        let guard = state.begin().unwrap();
        assert!(state.is_active());
        assert_eq!(state.begin().err().as_deref(), Some("Already running"));
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
    fn stop_only_works_while_recording() {
        let state = RunState::default();
        let _guard = state.begin().unwrap();
        assert!(state.request_stop().is_err());
        assert!(!state.interrupt.load(Ordering::SeqCst));
        state.set_recording(true);
        assert!(state.request_stop().is_ok());
        assert!(state.interrupt.load(Ordering::SeqCst));
    }

    #[test]
    fn last_session_round_trips() {
        let state = RunState::default();
        assert_eq!(state.last_session(), None);
        state.set_last_session(Some(PathBuf::from("/notes/2026-10-22_0900")));
        assert_eq!(
            state.last_session(),
            Some(PathBuf::from("/notes/2026-10-22_0900"))
        );
        state.set_last_session(None);
        assert_eq!(state.last_session(), None);
    }
}
