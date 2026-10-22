//! `record` with fake capture commands (shell one-liners that print raw PCM).
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use mn_core::session::{SessionInfo, StopReason};
use mn_core::{record, Config, RecordOptions, Status};

/// 0.5 s of digital silence, then 1 s of non-zero PCM (every byte 0x40), then stay alive without
/// output. The silence first gives the adaptive noise floor something to compare the sound to.
const LOUD_THEN_IDLE: &str =
    "head -c 16000 /dev/zero; head -c 32000 /dev/zero | tr '\\0' '@'; exec sleep 30";
/// 1 s of digital silence, then stay alive without output.
const SILENT_THEN_IDLE: &str = "head -c 32000 /dev/zero; exec sleep 30";
const IDLE: &str = "exec sleep 30";

fn sh(script: &str) -> Vec<String> {
    ["sh", "-c", script].map(String::from).to_vec()
}

fn config(notes: &Path, me: &str, them: &str) -> Config {
    Config {
        notes_dir: notes.to_path_buf(),
        capture_me: Some(sh(me)),
        capture_them: Some(sh(them)),
        silence_seconds: 1,
        silence_threshold_db: -50.0,
        ..Config::default()
    }
}

fn run(cfg: &Config, opts: &RecordOptions) -> (anyhow::Result<PathBuf>, Vec<Status>) {
    let statuses = RefCell::new(Vec::new());
    let result = record(cfg, opts, |s| statuses.borrow_mut().push(s));
    (result, statuses.into_inner())
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

fn pid_is_gone(pid_file: &Path) -> bool {
    let pid = fs::read_to_string(pid_file).unwrap();
    !Path::new("/proc").join(pid.trim()).exists()
}

#[test]
fn stops_on_silence_after_speech_and_writes_session_files() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), LOUD_THEN_IDLE, SILENT_THEN_IDLE);
    let started = Instant::now();

    let (result, statuses) = run(&cfg, &RecordOptions::default());
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
    assert_eq!(info.stop_reason, StopReason::Silence);
    assert_eq!((info.seconds.me, info.seconds.them), (1.5, 1.0));
    assert!(!info.audio_deleted);
    let started_at = chrono::DateTime::parse_from_rfc3339(&info.started_at).unwrap();
    let ended_at = chrono::DateTime::parse_from_rfc3339(&info.ended_at).unwrap();
    assert!(ended_at >= started_at);
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(session.join("session.json")).unwrap()).unwrap();
    assert_eq!(raw["stop_reason"], "silence");

    assert!(statuses
        .iter()
        .any(|s| matches!(s, Status::Recording { me_db, .. } if *me_db > -10.0)));
    assert_eq!(
        statuses.last(),
        Some(&Status::Stopped {
            session,
            reason: StopReason::Silence
        })
    );
}

#[test]
fn quiet_call_never_auto_stops_before_first_speech_and_children_are_killed() {
    let notes = tempfile::tempdir().unwrap();
    let pid_file = notes.path().join("me.pid");
    let me = format!("echo $$ > '{}'; {IDLE}", pid_file.display());
    let cfg = config(notes.path(), &me, IDLE);
    let opts = RecordOptions {
        max_seconds: Some(2),
        ..RecordOptions::default()
    };

    let (result, statuses) = run(&cfg, &opts);
    let session = result.unwrap();

    let info = SessionInfo::load(&session).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Max);
    assert!(info.seconds.me < 0.1 && info.seconds.them < 0.1);
    assert!(statuses.iter().any(|s| matches!(
        s,
        Status::Recording {
            elapsed: 1,
            silent_for: 0,
            ..
        }
    )));
    assert!(pid_is_gone(&pid_file), "capture process must be killed");
}

#[test]
fn stop_file_stops_with_user_reason() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), IDLE, IDLE);
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
    let cfg = config(notes.path(), IDLE, IDLE);
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

#[test]
fn stops_with_capture_ended_when_both_captures_exit() {
    let notes = tempfile::tempdir().unwrap();
    let silent = "head -c 64000 /dev/zero";
    let cfg = config(notes.path(), silent, silent);

    let (result, statuses) = run(&cfg, &RecordOptions::default());
    let session = result.unwrap();

    assert_eq!(wav_samples(&session.join("me.wav")).len(), 32_000);
    assert_eq!(wav_samples(&session.join("them.wav")).len(), 32_000);
    let info = SessionInfo::load(&session).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::CaptureEnded);
    assert_eq!((info.seconds.me, info.seconds.them), (2.0, 2.0));
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(session.join("session.json")).unwrap()).unwrap();
    assert_eq!(raw["stop_reason"], "capture_ended");
    let last = serde_json::to_value(statuses.last().unwrap()).unwrap();
    assert_eq!(last["state"], "stopped");
    assert_eq!(last["reason"], "capture_ended");
}

#[test]
fn captures_exiting_after_speech_is_capture_ended_not_silence() {
    let notes = tempfile::tempdir().unwrap();
    let speech = "head -c 16000 /dev/zero; head -c 32000 /dev/zero | tr '\\0' '@'";
    let cfg = config(notes.path(), speech, "head -c 48000 /dev/zero");

    let (result, _) = run(&cfg, &RecordOptions::default());

    let info = SessionInfo::load(&result.unwrap()).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::CaptureEnded);
}

#[test]
fn one_capture_exiting_does_not_stop_the_recording() {
    let notes = tempfile::tempdir().unwrap();
    let cfg = config(notes.path(), "true", IDLE);
    let opts = RecordOptions {
        max_seconds: Some(1),
        ..RecordOptions::default()
    };

    let (result, _) = run(&cfg, &opts);

    let info = SessionInfo::load(&result.unwrap()).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Max);
}

#[test]
fn noisy_mic_above_the_fixed_threshold_still_auto_stops() {
    let notes = tempfile::tempdir().unwrap();
    // Constant room noise at 0x0101 = -42 dBFS (above the -50 threshold), a 0.25 s burst at -6 dBFS,
    // then noise forever, paced in real time (100 ms per chunk).
    let me = "head -c 48000 /dev/zero | tr '\\0' '\\1'; \
              head -c 8000 /dev/zero | tr '\\0' '@'; \
              while true; do head -c 3200 /dev/zero | tr '\\0' '\\1'; sleep 0.1; done";
    let cfg = config(notes.path(), me, IDLE);
    let opts = RecordOptions {
        max_seconds: Some(20),
        ..RecordOptions::default()
    };
    let started = Instant::now();

    let (result, _) = run(&cfg, &opts);

    let info = SessionInfo::load(&result.unwrap()).unwrap().unwrap();
    assert_eq!(info.stop_reason, StopReason::Silence);
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn odd_chunk_boundaries_do_not_shift_samples() {
    let notes = tempfile::tempdir().unwrap();
    // 3 bytes, a pause, then 3 more: samples 0x4040, 0x4040, 0x4040 only if the odd byte is carried over
    let pcm = "printf '@@@'; sleep 0.3; printf '@@@'";
    let cfg = config(notes.path(), pcm, "true");

    let (result, _) = run(&cfg, &RecordOptions::default());

    let me = wav_samples(&result.unwrap().join("me.wav"));
    assert_eq!(me, vec![0x4040; 3]);
}

/// True if a process runs exactly `sh -c <script>` or, once the shell has exec'd, `sleep <secs>`.
fn capture_running(script: &str, sleep_secs: &str) -> bool {
    let as_sh = format!("sh\0-c\0{script}\0").into_bytes();
    let as_sleep = format!("sleep\0{sleep_secs}\0").into_bytes();
    fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| fs::read(e.ok()?.path().join("cmdline")).ok())
        .any(|cmdline| cmdline == as_sh || cmdline == as_sleep)
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
