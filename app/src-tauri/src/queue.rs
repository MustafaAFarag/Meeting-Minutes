use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use mn_core::Status;

/// What the widget shows about the notes queue. Sent to the window as the `mn-queue` event.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct QueueSnapshot {
    /// The stage of the session being processed now, if any.
    pub processing: Option<String>,
    /// Sessions waiting behind it.
    pub waiting: usize,
    /// The latest failure that has not been retried yet.
    pub last_error: Option<String>,
}

/// A session that failed. It stays here so "Try again" can queue it once more.
struct Failure {
    session: PathBuf,
    message: String,
}

#[derive(Default)]
struct Inner {
    waiting: VecDeque<PathBuf>,
    current: Option<PathBuf>,
    label: Option<String>,
    failed: Vec<Failure>,
}

/// Recorded sessions waiting for transcribe -> summarize -> publish. One worker thread takes them
/// in the order they were added, one at a time.
#[derive(Default)]
pub struct JobQueue {
    inner: Mutex<Inner>,
    wake: Condvar,
}

/// The stage shown before the core reports its own.
const FIRST_LABEL: &str = "Writing notes";

impl JobQueue {
    /// Adds a session at the back. A session that is already waiting or being processed is
    /// not added twice.
    pub fn push(&self, session: PathBuf) {
        let mut inner = self.lock();
        if inner.current.as_ref() == Some(&session) || inner.waiting.contains(&session) {
            return;
        }
        inner.waiting.push_back(session);
        self.wake.notify_one();
    }

    /// Blocks until at least one session is waiting.
    pub fn wait_for_work(&self) {
        let mut inner = self.lock();
        while inner.waiting.is_empty() {
            inner = self
                .wake
                .wait(inner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Sets the stage shown for the session being processed.
    pub fn set_label(&self, label: &str) {
        self.lock().label = Some(label.to_string());
    }

    /// Moves every failed session back to the end of the queue. Returns how many.
    pub fn retry_failed(&self) -> usize {
        let mut inner = self.lock();
        let failed = std::mem::take(&mut inner.failed);
        let count = failed.len();
        for failure in failed {
            if !inner.waiting.contains(&failure.session) {
                inner.waiting.push_back(failure.session);
            }
        }
        if count > 0 {
            self.wake.notify_one();
        }
        count
    }

    pub fn snapshot(&self) -> QueueSnapshot {
        let inner = self.lock();
        QueueSnapshot {
            processing: inner.current.as_ref().and(inner.label.clone()),
            waiting: inner.waiting.len(),
            last_error: inner.failed.last().map(|f| f.message.clone()),
        }
    }

    /// Takes the oldest waiting session and runs `process` on it. A failure (or a panic) is
    /// kept for "Try again" and does not stop the sessions behind it. `changed` runs after the
    /// session is taken and again after it is finished. Returns false when nothing was waiting.
    pub fn work_one(
        &self,
        process: impl FnOnce(&Path) -> Result<(), String>,
        changed: impl Fn(),
    ) -> bool {
        let session = {
            let mut inner = self.lock();
            let Some(session) = inner.waiting.pop_front() else {
                return false;
            };
            inner.current = Some(session.clone());
            inner.label = Some(FIRST_LABEL.to_string());
            session
        };
        changed();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| process(&session)))
            .unwrap_or_else(|_| Err("The app hit an unexpected problem.".to_string()));
        {
            let mut inner = self.lock();
            inner.current = None;
            inner.label = None;
            if let Err(message) = result {
                eprintln!("error: {message}");
                inner.failed.push(Failure { session, message });
            }
        }
        changed();
        true
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The stage a core status starts, if it starts one.
pub fn stage_label(status: &Status) -> Option<&'static str> {
    match status {
        Status::Transcribing { .. } => Some("Transcribing"),
        Status::Summarizing { .. } => Some("Writing notes"),
        Status::Publishing { .. } => Some("Updating My Meetings"),
        _ => None,
    }
}

/// Sessions in `notes_dir` that were recorded but never finished processing, oldest first.
/// A session counts when it still has its audio (and `session.json`, which is only written once
/// the recording closed cleanly) or a transcript, and has no `card.json`. `skip` is the session
/// being recorded right now, if any.
pub fn unfinished_sessions(notes_dir: &Path, skip: Option<&Path>) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(notes_dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|dir| dir.is_dir() && Some(dir.as_path()) != skip)
        .filter(|dir| {
            let hidden = dir
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'));
            !hidden && is_unfinished(dir)
        })
        .collect();
    // Session names are `YYYY-MM-DD_HHMM[-n]`, so the name order is the recording order.
    found.sort();
    found
}

fn is_unfinished(dir: &Path) -> bool {
    let has = |name: &str| dir.join(name).exists();
    let has_audio = has("session.json") && (has("me.wav") || has("them.wav"));
    (has_audio || has("transcript.md")) && !has("card.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RunState;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex as StdMutex;

    fn dir(name: &str) -> PathBuf {
        PathBuf::from("/notes").join(name)
    }

    /// Runs every waiting session through `Ok(())`, returning the order they came in.
    fn drain(queue: &JobQueue) -> Vec<PathBuf> {
        let seen = StdMutex::new(Vec::new());
        while queue.work_one(
            |d| {
                seen.lock().unwrap().push(d.to_path_buf());
                Ok(())
            },
            || {},
        ) {}
        seen.into_inner().unwrap()
    }

    #[test]
    fn sessions_are_processed_in_the_order_they_were_added() {
        let queue = JobQueue::default();
        queue.push(dir("a"));
        queue.push(dir("b"));
        queue.push(dir("c"));
        assert_eq!(drain(&queue), vec![dir("a"), dir("b"), dir("c")]);
        assert_eq!(queue.snapshot().waiting, 0);
    }

    #[test]
    fn a_failed_session_does_not_block_the_next_one() {
        let queue = JobQueue::default();
        queue.push(dir("bad"));
        queue.push(dir("good"));
        assert!(queue.work_one(|_| Err("whisper broke".into()), || {}));
        let done = AtomicUsize::new(0);
        assert!(queue.work_one(
            |d| {
                assert_eq!(d, dir("good"));
                done.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            || {}
        ));
        assert_eq!(done.load(Ordering::SeqCst), 1);
        let snap = queue.snapshot();
        assert_eq!(snap.last_error.as_deref(), Some("whisper broke"));
        assert_eq!((snap.processing, snap.waiting), (None, 0));
        assert!(!queue.work_one(|_| Ok(()), || {}));
    }

    #[test]
    fn a_panic_counts_as_a_failure_and_the_queue_goes_on() {
        let queue = JobQueue::default();
        queue.push(dir("a"));
        queue.push(dir("b"));
        assert!(queue.work_one(|_| panic!("boom"), || {}));
        assert!(queue.snapshot().last_error.is_some());
        assert!(queue.work_one(|_| Ok(()), || {}));
    }

    #[test]
    fn try_again_requeues_the_failed_sessions_and_clears_the_error() {
        let queue = JobQueue::default();
        queue.push(dir("a"));
        queue.push(dir("b"));
        queue.work_one(|_| Err("one".into()), || {});
        queue.work_one(|_| Err("two".into()), || {});
        assert_eq!(queue.snapshot().last_error.as_deref(), Some("two"));
        assert_eq!(queue.retry_failed(), 2);
        let snap = queue.snapshot();
        assert_eq!((snap.waiting, snap.last_error), (2, None));
        assert_eq!(drain(&queue), vec![dir("a"), dir("b")]);
        assert_eq!(queue.retry_failed(), 0);
    }

    #[test]
    fn the_same_session_is_never_queued_twice() {
        let queue = JobQueue::default();
        queue.push(dir("a"));
        queue.push(dir("a"));
        assert_eq!(queue.snapshot().waiting, 1);
        queue.work_one(
            |_| {
                // Queued again while it is being processed.
                queue.push(dir("a"));
                Ok(())
            },
            || {},
        );
        assert_eq!(queue.snapshot().waiting, 0);
    }

    #[test]
    fn the_snapshot_shows_the_stage_and_the_sessions_behind_it() {
        let queue = JobQueue::default();
        queue.push(dir("a"));
        queue.push(dir("b"));
        let during = StdMutex::new(None);
        queue.work_one(
            |_| {
                queue.set_label("Transcribing");
                *during.lock().unwrap() = Some(queue.snapshot());
                Ok(())
            },
            || {},
        );
        let during = during.into_inner().unwrap().unwrap();
        assert_eq!(during.processing.as_deref(), Some("Transcribing"));
        assert_eq!(during.waiting, 1);
        assert_eq!(queue.snapshot().processing, None);
    }

    #[test]
    fn changed_runs_after_taking_and_after_finishing() {
        let queue = JobQueue::default();
        queue.push(dir("a"));
        let seen = StdMutex::new(Vec::new());
        queue.work_one(
            |_| Ok(()),
            || seen.lock().unwrap().push(queue.snapshot().processing),
        );
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 2);
        assert!(seen[0].is_some());
        assert_eq!(seen[1], None);
    }

    #[test]
    fn recording_can_start_while_notes_are_being_written() {
        let state = RunState::default();
        state.queue.push(dir("a"));
        let ran = state.queue.work_one(
            |_| {
                // Processing is running: a new recording is allowed, a second one is not.
                let guard = state.begin().expect("start while processing");
                assert_eq!(state.begin().err().as_deref(), Some("Already recording"));
                drop(guard);
                Ok(())
            },
            || {},
        );
        assert!(ran);
    }

    #[test]
    fn processing_does_not_start_while_the_slot_is_free_or_held() {
        // Holding the recording slot does not stop the queue either.
        let state = RunState::default();
        let _recording = state.begin().unwrap();
        state.queue.push(dir("a"));
        assert!(state.queue.work_one(|_| Ok(()), || {}));
    }

    #[test]
    fn stage_labels_follow_the_core_status() {
        use mn_core::capture::Track;
        let transcribing = Status::Transcribing { track: Track::Me };
        assert_eq!(stage_label(&transcribing), Some("Transcribing"));
        let publishing = Status::Publishing { meetings: 3 };
        assert_eq!(stage_label(&publishing), Some("Updating My Meetings"));
        let recording = Status::Recording {
            elapsed: 1,
            me_db: 0.0,
            them_db: 0.0,
        };
        assert_eq!(stage_label(&recording), None);
    }

    fn session(root: &Path, name: &str, files: &[&str]) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        for f in files {
            fs::write(dir.join(f), b"x").unwrap();
        }
    }

    fn names(found: Vec<PathBuf>) -> Vec<String> {
        found
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn recovery_picks_sessions_that_never_got_a_card_oldest_first() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        session(
            r,
            "2026-10-22_1100",
            &["session.json", "me.wav", "them.wav"],
        );
        session(r, "2026-10-22_0900", &["session.json", "transcript.md"]);
        session(r, "2026-10-22_0900-2", &["session.json", "them.wav"]);
        // Finished: has its card (even if publishing is still pending).
        session(
            r,
            "2026-10-21_1000",
            &["session.json", "transcript.md", "card.json"],
        );
        session(
            r,
            "2026-10-21_1100",
            &[
                "session.json",
                "card.json",
                "meeting.json",
                "PUBLISH_PENDING",
            ],
        );
        // Nothing left to process.
        session(r, "2026-10-20_1000", &["session.json"]);
        assert_eq!(
            names(unfinished_sessions(r, None)),
            ["2026-10-22_0900", "2026-10-22_0900-2", "2026-10-22_1100"]
        );
    }

    #[test]
    fn recovery_skips_the_session_being_recorded_and_unclosed_recordings() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        // A recording in progress (or one that crashed) has audio but no session.json yet.
        session(r, "2026-10-22_0900", &["me.wav", "them.wav"]);
        session(r, "2026-10-22_1000", &["session.json", "me.wav"]);
        session(r, "2026-10-22_1100", &["session.json", "transcript.md"]);
        let skip = r.join("2026-10-22_1100");
        assert_eq!(
            names(unfinished_sessions(r, Some(&skip))),
            ["2026-10-22_1000"]
        );
    }

    #[test]
    fn recovery_ignores_files_hidden_dirs_and_a_missing_notes_dir() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        session(r, ".staging", &["session.json", "me.wav"]);
        fs::write(r.join("me.wav"), b"x").unwrap();
        assert!(unfinished_sessions(r, None).is_empty());
        assert!(unfinished_sessions(&r.join("missing"), None).is_empty());
    }
}
