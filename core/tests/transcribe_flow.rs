//! `transcribe` end to end with a fake whisper_cli (a shell script).
use std::cell::RefCell;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mn_core::capture::Track;
use mn_core::session::{SessionInfo, StopReason, TrackSeconds};
use mn_core::{transcribe, Config, Status};

// Writing an executable while another test forks can give ETXTBSY; run these one at a time.
static LOCK: Mutex<()> = Mutex::new(());

const THEM_JSON: &str = include_str!("fixtures/whisper-sample.json");
const ME_JSON: &str = r#"{"transcription":[
  {"offsets":{"from":0,"to":2000},"text":" Hi, can you hear me?"},
  {"offsets":{"from":2500,"to":3000},"text":" you"},
  {"offsets":{"from":20000,"to":21000},"text":" [BLANK_AUDIO]"},
  {"offsets":{"from":30000,"to":31000},"text":" Yes, go ahead."},
  {"offsets":{"from":40000,"to":41000},"text":" වවවවවවවවවවවවවවවවවවවව"}
]}"#;

struct Fixture {
    dir: tempfile::TempDir,
    log: PathBuf,
}

impl Fixture {
    fn session(&self) -> PathBuf {
        self.dir.path().join("session")
    }

    fn config(&self, whisper_script: &str) -> Config {
        let script = self.dir.path().join("fake-whisper");
        fs::write(&script, whisper_script).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        Config {
            whisper_cli: script,
            whisper_model: Some(PathBuf::from("/models/ggml-small.bin")),
            vocab_prompt: Some("CTCHealth, Skylink".to_string()),
            ..Config::default()
        }
    }

    /// Copies `<src>/<track>.json` to the `-of` path and logs its arguments.
    fn copying_whisper(&self) -> String {
        let src = self.dir.path().join("whisper-out");
        format!(
            "#!/bin/sh\necho \"$@\" >> '{log}'\nwhile [ $# -gt 0 ]; do\n  [ \"$1\" = -of ] && of=$2\n  shift\ndone\ncp '{src}'/\"$(basename \"$of\")\".json \"$of.json\"\n",
            log = self.log.display(),
            src = src.display()
        )
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

fn write_wav(path: &Path, samples: &[i16]) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for &s in samples {
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();
}

/// `burst_samples` of a +-`amplitude` square wave, then `silence_secs` of digital silence.
fn square_then_silence(amplitude: i16, burst_samples: usize, silence_secs: usize) -> Vec<i16> {
    let burst = (0..burst_samples).map(|i| if i % 2 == 0 { amplitude } else { -amplitude });
    burst
        .chain(std::iter::repeat_n(0, silence_secs * 16_000))
        .collect()
}

fn audio(loud: bool) -> Vec<i16> {
    square_then_silence(if loud { 8000 } else { 0 }, 16_000, 0)
}

fn fixture(me_loud: bool, them_loud: bool) -> Fixture {
    fixture_with(&audio(me_loud), &audio(them_loud))
}

fn fixture_with(me: &[i16], them: &[i16]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let session = dir.path().join("session");
    fs::create_dir(&session).unwrap();
    write_wav(&session.join("me.wav"), me);
    write_wav(&session.join("them.wav"), them);
    SessionInfo {
        started_at: "2026-10-22T10:49:05+02:00".to_string(),
        ended_at: "2026-10-22T11:32:40+02:00".to_string(),
        stop_reason: StopReason::Silence,
        seconds: TrackSeconds { me: 1.0, them: 1.0 },
        audio_deleted: false,
    }
    .save(&session)
    .unwrap();
    let out = dir.path().join("whisper-out");
    fs::create_dir(&out).unwrap();
    fs::write(out.join("me.json"), ME_JSON).unwrap();
    fs::write(out.join("them.json"), THEM_JSON).unwrap();
    let log = dir.path().join("calls.log");
    Fixture { dir, log }
}

fn run(fx: &Fixture, cfg: &Config) -> (anyhow::Result<PathBuf>, Vec<Status>) {
    let statuses = RefCell::new(Vec::new());
    let result = transcribe(cfg, &fx.session(), |s| statuses.borrow_mut().push(s));
    (result, statuses.into_inner())
}

#[test]
fn writes_transcript_then_deletes_audio() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(true, true);
    let cfg = fx.config(&fx.copying_whisper());

    let (result, statuses) = run(&fx, &cfg);
    let transcript = result.unwrap();

    assert_eq!(transcript, fx.session().join("transcript.md"));
    let md = fs::read_to_string(&transcript).unwrap();
    assert_eq!(
        md,
        "# Transcript 2026-10-22 10:49–11:32\n\n\
         [00:00:00] **Me:** Hi, can you hear me?\n\
         [00:00:00] **Them:** And so my fellow Americans, ask not what your country can do for you, ask what you can do for your country.\n\
         [00:00:30] **Me:** Yes, go ahead.\n",
        "hallucinations and the token-loop garbage line are dropped"
    );

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fx.session().join("transcript.json")).unwrap())
            .unwrap();
    assert_eq!(json.as_array().unwrap().len(), 3);
    assert_eq!(json[0]["speaker"], "me");
    assert_eq!(json[0]["start_ms"], 0);
    assert_eq!(json[0]["end_ms"], 2000);
    assert_eq!(json[2]["text"], "Yes, go ahead.");

    for gone in ["me.wav", "them.wav", "me.json", "them.json"] {
        assert!(
            !fx.session().join(gone).exists(),
            "{gone} should be deleted"
        );
    }
    assert!(
        SessionInfo::load(&fx.session())
            .unwrap()
            .unwrap()
            .audio_deleted
    );

    assert_eq!(
        statuses,
        vec![
            Status::Transcribing { track: Track::Me },
            Status::Transcribing { track: Track::Them },
            Status::Transcribed { transcript },
        ]
    );
}

#[test]
fn passes_model_language_output_and_prompt_to_whisper() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(false, true);
    let cfg = fx.config(&fx.copying_whisper());
    run(&fx, &cfg).0.unwrap();

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "silent me track must not be transcribed");
    let call = &calls[0];
    let them = fx.session().join("them");
    assert!(call.contains("-m /models/ggml-small.bin"), "{call}");
    assert!(
        call.contains(&format!("-f {}.wav", them.display())),
        "{call}"
    );
    assert!(call.contains("-l auto -oj"), "{call}");
    assert!(call.contains(&format!("-of {}", them.display())), "{call}");
    assert!(call.contains("-np"), "{call}");
    assert!(call.contains("--prompt CTCHealth, Skylink"), "{call}");
}

#[test]
fn passes_threads_language_and_leaves_vad_off_by_default() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(false, true);
    let mut cfg = fx.config(&fx.copying_whisper());
    cfg.threads = 10;
    run(&fx, &cfg).0.unwrap();

    let call = &fx.calls()[0];
    assert!(call.contains("-t 10"), "{call}");
    assert!(call.contains("-l auto -oj"), "{call}");
    assert!(!call.contains("--vad"), "{call}");
    assert!(!call.contains("-vm"), "{call}");
}

#[test]
fn passes_configured_language_and_vad_model() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(false, true);
    let mut cfg = fx.config(&fx.copying_whisper());
    cfg.language = "ar".to_string();
    cfg.vad_model = Some(PathBuf::from("/models/silero-vad.bin"));
    cfg.threads = 3;
    run(&fx, &cfg).0.unwrap();

    let call = &fx.calls()[0];
    assert!(call.contains("-l ar -oj"), "{call}");
    assert!(call.contains("-t 3"), "{call}");
    assert!(call.contains("--vad -vm /models/silero-vad.bin"), "{call}");
}

#[test]
fn default_thread_count_is_passed_to_whisper() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(false, true);
    let cfg = fx.config(&fx.copying_whisper());
    run(&fx, &cfg).0.unwrap();

    let expected = std::thread::available_parallelism().unwrap().get();
    let call = &fx.calls()[0];
    assert!(call.contains(&format!("-t {expected}")), "{call}");
}

#[test]
fn quiet_speaker_in_a_long_file_is_still_transcribed() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 0.2 s at -38 dBFS then 60 s of silence: whole-file RMS is about -63 dB, below the -50 threshold
    let me = square_then_silence(400, 3200, 60);
    let fx = fixture_with(&me, &audio(false));
    let cfg = fx.config(&fx.copying_whisper());

    let (result, statuses) = run(&fx, &cfg);
    result.unwrap();

    assert_eq!(statuses[0], Status::Transcribing { track: Track::Me });
    assert_eq!(fx.calls().len(), 1);
}

#[test]
fn track_with_no_window_above_the_threshold_is_skipped() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // +-100 square wave is -50.3 dBFS: every window is just under -50
    let me = square_then_silence(100, 16_000, 1);
    let fx = fixture_with(&me, &audio(true));
    let cfg = fx.config(&fx.copying_whisper());

    run(&fx, &cfg).0.unwrap();

    let calls = fx.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].contains("/them.wav"), "{}", calls[0]);
}

#[test]
fn omits_prompt_when_not_configured() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(false, true);
    let mut cfg = fx.config(&fx.copying_whisper());
    cfg.vocab_prompt = None;
    run(&fx, &cfg).0.unwrap();
    assert!(!fx.calls()[0].contains("--prompt"));
}

#[test]
fn whisper_failure_keeps_audio_and_errors() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(true, true);
    let cfg = fx.config("#!/bin/sh\necho 'model not found' >&2\nexit 1\n");

    let (result, _) = run(&fx, &cfg);
    let err = format!("{:#}", result.unwrap_err());

    assert!(err.contains("model not found"), "{err}");
    assert!(fx.session().join("me.wav").exists());
    assert!(fx.session().join("them.wav").exists());
    assert!(!fx.session().join("transcript.md").exists());
    assert!(
        !SessionInfo::load(&fx.session())
            .unwrap()
            .unwrap()
            .audio_deleted
    );
}

#[test]
fn unparseable_whisper_output_keeps_audio() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(true, true);
    fs::write(fx.dir.path().join("whisper-out/me.json"), "not json").unwrap();
    let cfg = fx.config(&fx.copying_whisper());

    assert!(run(&fx, &cfg).0.is_err());
    assert!(fx.session().join("me.wav").exists());
    assert!(!fx.session().join("transcript.md").exists());
}

#[test]
fn all_silent_session_gets_header_only_transcript() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(false, false);
    let cfg = fx.config(&fx.copying_whisper());

    let (result, _) = run(&fx, &cfg);
    let md = fs::read_to_string(result.unwrap()).unwrap();

    assert_eq!(md, "# Transcript 2026-10-22 10:49–11:32\n");
    assert!(fx.calls().is_empty());
    assert!(!fx.session().join("me.wav").exists());
}

#[test]
fn missing_model_is_a_clear_error() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(true, true);
    let mut cfg = fx.config("#!/bin/sh\nexit 0\n");
    cfg.whisper_model = None;

    let err = run(&fx, &cfg).0.unwrap_err().to_string();
    assert!(err.contains("whisper_model"), "{err}");
    assert!(fx.session().join("me.wav").exists());
}

#[test]
fn session_without_audio_is_an_error() {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let fx = fixture(true, true);
    fs::remove_file(fx.session().join("me.wav")).unwrap();
    fs::remove_file(fx.session().join("them.wav")).unwrap();
    let cfg = fx.config("#!/bin/sh\nexit 0\n");

    assert!(run(&fx, &cfg).0.is_err());
}
