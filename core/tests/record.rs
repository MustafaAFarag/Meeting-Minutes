//! `record` with fake capture commands (shell one-liners that print raw PCM).
#![cfg(unix)]

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod common;

use mn_core::session::{SessionInfo, StopReason};
use mn_core::{record, Config, ErrorKind, Failure, RecordOptions, Status};

/// 0.5 s of digital silence, then 1 s of non-zero PCM (every byte 0x40), then stay alive without
/// output.
const LOUD_THEN_IDLE: &str =
    "head -c 16000 /dev/zero; head -c 32000 /dev/zero | tr '\\0' '@'; exec sleep 30";
/// 1 s of digital silence, then stay alive without output.
const SILENT_THEN_IDLE: &str = "head -c 32000 /dev/zero; exec sleep 30";
const IDLE: &str = "exec sleep 30";
static LOG_ENV_LOCK: Mutex<()> = Mutex::new(());

struct LogEnv(Option<std::ffi::OsString>);

impl LogEnv {
    fn set(path: &Path) -> Self {
        common::use_test_log_dir();
        let previous = std::env::var_os("MN_LOG_DIR");
        std::env::set_var("MN_LOG_DIR", path);
        Self(previous)
    }
}

impl Drop for LogEnv {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("MN_LOG_DIR", value),
            None => std::env::remove_var("MN_LOG_DIR"),
        }
    }
}

fn sh(script: &str) -> Vec<String> {
    ["sh", "-c", script].map(String::from).to_vec()
}

fn config(notes: &Path, me: &str, them: &str) -> Config {
    Config {
        notes_dir: notes.to_path_buf(),
        capture_me: Some(sh(me)),
        capture_them: Some(sh(them)),
        ..Config::default()
    }
}

fn run(cfg: &Config, opts: &RecordOptions) -> (anyhow::Result<PathBuf>, Vec<Status>) {
    common::use_test_log_dir();
    let statuses = RefCell::new(Vec::new());
    let result = record(cfg, opts, |s| statuses.borrow_mut().push(s));
    (result, statuses.into_inner())
}

fn assert_no_sound(err: &anyhow::Error) {
    assert_eq!(
        err.downcast_ref::<Failure>().map(|failure| failure.kind),
        Some(ErrorKind::NoSound),
        "{err:#}"
    );
}

fn wav_samples(path: &Path) -> Vec<i16> {
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (1, 16_000, 16)
    );
    reader.samples::<i16>().map(|s| s.unwrap()).collect()
}

fn only_session_dir(notes: &Path) -> Option<PathBuf> {
    fs::read_dir(notes)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir())
}

/// `kill -0` instead of /proc, which macOS does not have.
fn pid_is_gone(pid_file: &Path) -> bool {
    let pid = fs::read_to_string(pid_file).unwrap();
    !Command::new("sh")
        .args(["-c", &format!("kill -0 {}", pid.trim())])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn writes_session_files_and_stops_at_max_seconds() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), LOUD_THEN_IDLE, SILENT_THEN_IDLE);
    let opts = RecordOptions {
        max_seconds: Some(2),
        ..RecordOptions::default()
    };
    let started = Instant::now();

    let (result, statuses) = run(&cfg, &opts);
    let session = result.unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "should stop long before the sleep ends"
    );
    let name = session.file_name().unwrap().to_str().unwrap();
    assert_eq!(session.parent().unwrap(), notes.path());
    let b = name.as_bytes();
    assert!(
        name.len() == 15 && b[4] == b'-' && b[7] == b'-' && b[10] == b'_',
        "{name}"
    );

    let me = wav_samples(&session.join("me.wav"));
    assert_eq!(me.len(), 24_000);
    assert!(me[..8_000].iter().all(|&s| s == 0));
    assert!(me[8_000..].iter().all(|&s| s == 0x4040));
    let them = wav_samples(&session.join("them.wav"));
    assert_eq!(them.len(), 16_000);
    assert!(them.iter().all(|&s| s == 0));

    let info = SessionInfo::load(&session).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Max);
    assert_eq!((info.seconds.me, info.seconds.them), (1.5, 1.0));
    assert!(!info.audio_deleted);
    let started_at = chrono::DateTime::parse_from_rfc3339(&info.started_at).unwrap();
    let ended_at = chrono::DateTime::parse_from_rfc3339(&info.ended_at).unwrap();
    assert!(ended_at >= started_at);
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(session.join("session.json")).unwrap()).unwrap();
    assert_eq!(raw["stop_reason"], "max");

    assert!(statuses
        .iter()
        .any(|s| matches!(s, Status::Recording { me_db, .. } if *me_db > -10.0)));
    assert_eq!(
        statuses.last(),
        Some(&Status::Stopped {
            session,
            reason: StopReason::Max
        })
    );
}

#[test]
fn capture_processes_are_killed_when_recording_stops() {
    let notes = tempfile::tempdir().unwrap();
    let pid_file = notes.path().join("me.pid");
    let me = format!("echo $$ > '{}'; {IDLE}", pid_file.display());
    let cfg = config(notes.path(), &me, IDLE);
    let opts = RecordOptions {
        max_seconds: Some(2),
        ..RecordOptions::default()
    };

    let (result, statuses) = run(&cfg, &opts);
    let error = result.unwrap_err();
    assert_no_sound(&error);
    let session = only_session_dir(notes.path()).unwrap();

    let info = SessionInfo::load(&session).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Max);
    assert!(info.seconds.me < 0.1 && info.seconds.them < 0.1);
    assert!(!statuses
        .iter()
        .any(|status| matches!(status, Status::Stopped { .. })));
    assert!(pid_is_gone(&pid_file), "capture process must be killed");
}

#[test]
fn capture_processes_get_sigterm_before_being_killed() {
    let notes = tempfile::tempdir().unwrap();
    let marker = notes.path().join("got-term");
    let me = format!(
        "trap 'echo term > \"{}\"; exit 0' TERM; while true; do head -c 3200 /dev/zero; sleep 0.1; done",
        marker.display()
    );
    let cfg = config(notes.path(), &me, IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };
    let result = run(&cfg, &opts).0.unwrap_err();
    assert_no_sound(&result);
    assert!(
        marker.exists(),
        "the capture command must get SIGTERM so it can clean up"
    );
}

#[test]
fn stop_file_stops_with_user_reason() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), LOUD_THEN_IDLE, IDLE);
    let notes_path = notes.path().to_path_buf();
    let stopper = std::thread::spawn(move || loop {
        if let Some(dir) = only_session_dir(&notes_path) {
            std::thread::sleep(Duration::from_millis(400));
            fs::write(dir.join("STOP"), "").unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    });

    let (result, _) = run(&cfg, &RecordOptions::default());
    stopper.join().unwrap();

    let info = SessionInfo::load(&result.unwrap()).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::User);
}

#[test]
fn interrupt_flag_stops_with_signal_reason() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), LOUD_THEN_IDLE, IDLE);
    let opts = RecordOptions::default();
    let flag = opts.interrupt.clone();
    let setter = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        flag.store(true, Ordering::SeqCst);
    });

    let (result, _) = run(&cfg, &opts);
    setter.join().unwrap();

    let info = SessionInfo::load(&result.unwrap()).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Signal);
}

/// Runs a recording whose mic speaks for 1 s and then goes silent while the captures stay alive.
/// `stop` runs on another thread 3.5 s after the session dir appears.
fn run_silent_call_until(
    stop: impl FnOnce(&Path, &RecordOptions) + Send + 'static,
) -> (SessionInfo, Vec<Status>) {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), LOUD_THEN_IDLE, SILENT_THEN_IDLE);
    let opts = RecordOptions::default();
    let thread_opts = opts.clone();
    let notes_path = notes.path().to_path_buf();
    let stopper = std::thread::spawn(move || loop {
        if let Some(dir) = only_session_dir(&notes_path) {
            // well past the 1 s of silence the old auto-stop needed
            std::thread::sleep(Duration::from_millis(3500));
            stop(&dir, &thread_opts);
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    });

    let (result, statuses) = run(&cfg, &opts);
    stopper.join().unwrap();
    (
        SessionInfo::load(&result.unwrap()).unwrap().unwrap(),
        statuses,
    )
}

#[test]
fn silent_input_does_not_stop_the_recording_by_itself_stop_file() {
    let (info, statuses) = run_silent_call_until(|dir, _| fs::write(dir.join("STOP"), "").unwrap());

    assert_eq!(info.stop_reason, StopReason::User);
    assert!(statuses
        .iter()
        .any(|s| matches!(s, Status::Recording { elapsed: 3, .. })));
}

#[test]
fn silent_input_does_not_stop_the_recording_by_itself_interrupt() {
    let (info, statuses) =
        run_silent_call_until(|_, opts| opts.interrupt.store(true, Ordering::SeqCst));

    assert_eq!(info.stop_reason, StopReason::Signal);
    assert!(statuses
        .iter()
        .any(|s| matches!(s, Status::Recording { elapsed: 3, .. })));
}

#[test]
fn recording_status_json_has_only_the_level_fields() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), LOUD_THEN_IDLE, SILENT_THEN_IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };

    let (result, statuses) = run(&cfg, &opts);
    result.unwrap();

    let recording = statuses
        .iter()
        .find(|s| matches!(s, Status::Recording { .. }))
        .expect("a recording status");
    let json = serde_json::to_value(recording).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["elapsed", "me_db", "state", "them_db"], "{json}");
}

#[test]
fn early_capture_ended_is_no_sound_instead_of_success() {
    let notes = tempfile::tempdir().unwrap();
    let silent = "head -c 64000 /dev/zero";
    let cfg = config(notes.path(), silent, silent);

    let (result, statuses) = run(&cfg, &RecordOptions::default());
    let err = result.unwrap_err();
    assert_no_sound(&err);
    let session = only_session_dir(notes.path()).unwrap();

    assert_eq!(wav_samples(&session.join("me.wav")).len(), 32_000);
    assert_eq!(wav_samples(&session.join("them.wav")).len(), 32_000);
    let info = SessionInfo::load(&session).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::CaptureEnded);
    assert_eq!((info.seconds.me, info.seconds.them), (2.0, 2.0));
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(session.join("session.json")).unwrap()).unwrap();
    assert_eq!(raw["stop_reason"], "capture_ended");
    assert!(
        statuses.is_empty(),
        "a failed recording has no stopped status"
    );
}

#[test]
fn capture_that_closes_stdout_is_stopped_when_the_recording_stops() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), "exec 1>&-; exec sleep 30", LOUD_THEN_IDLE);
    let opts = RecordOptions::default();
    let notes_path = notes.path().to_path_buf();
    let stopper = std::thread::spawn(move || loop {
        if let Some(dir) = only_session_dir(&notes_path) {
            std::thread::sleep(Duration::from_millis(300));
            fs::write(dir.join("STOP"), "").unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    });
    let started = Instant::now();

    let (result, _) = run(&cfg, &opts);
    stopper.join().unwrap();
    let session = result.unwrap();

    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(
        SessionInfo::load(&session).unwrap().unwrap().stop_reason,
        StopReason::User
    );
}

#[test]
fn microphone_permission_failure_is_reported_while_system_audio_keeps_recording() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(
        notes.path(),
        "echo 'Permission denied opening microphone' >&2; exit 1",
        LOUD_THEN_IDLE,
    );
    let opts = RecordOptions::default();
    let notes_path = notes.path().to_path_buf();
    let stopper = std::thread::spawn(move || loop {
        if let Some(dir) = only_session_dir(&notes_path) {
            std::thread::sleep(Duration::from_millis(300));
            fs::write(dir.join("STOP"), "").unwrap();
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    });

    let (result, _) = run(&cfg, &opts);
    stopper.join().unwrap();

    let err = result.unwrap_err();
    assert_eq!(
        err.downcast_ref::<Failure>().map(|failure| failure.kind),
        Some(ErrorKind::MicUnavailable),
        "{err:#}"
    );
}

#[test]
fn capture_ended_almost_at_once_is_no_sound_even_if_a_short_buffer_arrived() {
    let notes = tempfile::tempdir().unwrap();
    let speech = "head -c 16000 /dev/zero; head -c 32000 /dev/zero | tr '\\0' '@'";
    let cfg = config(notes.path(), speech, "head -c 48000 /dev/zero");

    let (result, _) = run(&cfg, &RecordOptions::default());
    let err = result.unwrap_err();
    assert_no_sound(&err);
    let info = SessionInfo::load(&only_session_dir(notes.path()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(info.stop_reason, StopReason::CaptureEnded);
}

#[test]
fn both_silent_tracks_fail_with_no_sound() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), IDLE, IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };

    let (result, _) = run(&cfg, &opts);

    let err = result.unwrap_err();
    assert_no_sound(&err);
}

#[test]
fn one_silent_track_warns_and_recording_succeeds() {
    let _guard = LOG_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let notes = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let _log_env = LogEnv::set(logs.path());
    let cfg = config(notes.path(), LOUD_THEN_IDLE, IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };

    let session = run(&cfg, &opts).0.unwrap();

    assert!(session.is_dir());
    let log = fs::read_to_string(logs.path().join("meeting-notes.log")).unwrap();
    assert!(log.contains("WARN  [record] track=them silent"), "{log}");
}

#[test]
fn failed_capture_logs_scrubbed_stderr_tail() {
    let _guard = LOG_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let notes = tempfile::tempdir().unwrap();
    let logs = tempfile::tempdir().unwrap();
    let _log_env = LogEnv::set(logs.path());
    let failed_me = "echo 'person@example.test https://client.test/private' >&2; exit 2";
    let cfg = config(notes.path(), failed_me, LOUD_THEN_IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };

    assert!(run(&cfg, &opts).0.is_ok());

    let log = fs::read_to_string(logs.path().join("meeting-notes.log")).unwrap();
    assert!(
        log.contains("[capture] track=me exit_code=2 stderr_bytes="),
        "{log}"
    );
    assert!(log.contains("<email>"), "{log}");
    assert!(log.contains("<url>"), "{log}");
    assert!(!log.contains("person@example.test"), "{log}");
    assert!(!log.contains("https://client.test"), "{log}");
}

#[test]
fn one_capture_exiting_does_not_stop_the_recording() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), "true", LOUD_THEN_IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };

    let (result, _) = run(&cfg, &opts);

    let info = SessionInfo::load(&result.unwrap()).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Max);
}

#[test]
fn odd_chunk_boundaries_do_not_shift_samples() {
    let notes = tempfile::tempdir().unwrap();
    // 3 bytes, a pause, then 3 more: samples 0x4040, 0x4040, 0x4040 only if the odd byte is carried over
    let pcm = "printf '@@@'; sleep 0.3; printf '@@@'; exec sleep 30";
    let cfg = config(notes.path(), pcm, LOUD_THEN_IDLE);

    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };
    let (result, _) = run(&cfg, &opts);

    let me = wav_samples(&result.unwrap().join("me.wav"));
    assert_eq!(me, vec![0x4040; 3]);
}

/// True if a process runs exactly `sh -c <script>` or, once the shell has exec'd, `sleep <secs>`.
/// Uses `ps`, not /proc, so it also runs on macOS.
fn capture_running(script: &str, sleep_secs: &str) -> bool {
    let as_sh = format!("sh -c {script}");
    let as_sleep = format!("sleep {sleep_secs}");
    let out = Command::new("ps")
        .args(["-A", "-o", "args="])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .any(|args| args == as_sh || args == as_sleep)
}

#[test]
fn failed_spawn_is_an_error_and_kills_the_capture_already_started() {
    let notes = tempfile::tempdir().unwrap();
    // the unique sleep length identifies this test's capture process
    let script = "exec sleep 29.123";
    let mut cfg = config(notes.path(), script, IDLE);
    cfg.capture_them = Some(vec!["/nonexistent/capture-binary".to_string()]);

    let (result, _) = run(&cfg, &RecordOptions::default());

    let err = format!("{:#}", result.unwrap_err());
    assert!(err.contains("/nonexistent/capture-binary"), "{err}");
    assert!(
        !capture_running(script, "29.123"),
        "first capture must not be orphaned"
    );
}
