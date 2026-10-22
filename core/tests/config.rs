use std::path::PathBuf;

use mn_core::config::{default_capture_me, default_capture_them, Config};

#[test]
fn empty_file_gives_defaults() {
    let cfg = Config::parse("").unwrap();
    assert_eq!(cfg, Config::default());
    assert_eq!(cfg.silence_threshold_db, -50.0);
    assert_eq!(cfg.silence_seconds, 120);
    assert_eq!(cfg.whisper_cli, PathBuf::from("whisper-cli"));
    assert!(cfg.notes_dir.ends_with("MeetingNotes"));
    assert!(cfg.whisper_model.is_none());
    assert!(cfg.capture_me.is_none(), "computed at runtime");
    assert!(cfg.capture_them.is_none(), "computed at runtime");
}

#[test]
fn default_capture_commands_use_parec_with_an_explicit_device() {
    assert_eq!(
        default_capture_me("alsa_input.usb-mic.analog-stereo"),
        [
            "parec",
            "-d",
            "alsa_input.usb-mic.analog-stereo",
            "--format=s16le",
            "--rate=16000",
            "--channels=1",
            "--raw",
            "--latency-msec=100"
        ]
    );
    assert_eq!(
        default_capture_them("alsa_output.pci.analog-stereo"),
        [
            "parec",
            "-d",
            "alsa_output.pci.analog-stereo.monitor",
            "--format=s16le",
            "--rate=16000",
            "--channels=1",
            "--raw",
            "--latency-msec=100"
        ]
    );
}

#[test]
fn configured_capture_commands_skip_pactl() {
    let cfg = Config::parse(
        "capture_me = [\"mac-sidecar\", \"--mic\"]\ncapture_them = [\"mac-sidecar\", \"--system\"]\n",
    )
    .unwrap();
    assert_eq!(cfg.capture_me_argv().unwrap(), ["mac-sidecar", "--mic"]);
    assert_eq!(
        cfg.capture_them_argv().unwrap(),
        ["mac-sidecar", "--system"]
    );
}

#[test]
fn partial_file_overrides_only_what_it_sets() {
    let cfg = Config::parse(
        "silence_seconds = 30\nwhisper_model = \"/m/ggml-small.bin\"\ncapture_them = [\"mac-sidecar\", \"--system\"]\n",
    )
    .unwrap();
    assert_eq!(cfg.silence_seconds, 30);
    assert_eq!(cfg.silence_threshold_db, -50.0);
    assert_eq!(cfg.whisper_model, Some(PathBuf::from("/m/ggml-small.bin")));
    assert_eq!(
        cfg.capture_them_argv().unwrap(),
        ["mac-sidecar", "--system"]
    );
}

#[test]
fn unknown_keys_are_rejected() {
    assert!(Config::parse("silense_seconds = 30\n").is_err());
}

#[test]
fn leading_tilde_expands_to_home() {
    let cfg = Config::parse("notes_dir = \"~/Notes\"\n").unwrap();
    assert_eq!(cfg.notes_dir, dirs::home_dir().unwrap().join("Notes"));
}

#[test]
fn effective_config_prints_as_toml_and_round_trips() {
    let cfg = Config {
        whisper_model: Some(PathBuf::from("/m/ggml-small.bin")),
        vocab_prompt: Some("Skylink".to_string()),
        ..Config::default()
    };
    assert_eq!(Config::parse(&cfg.to_toml().unwrap()).unwrap(), cfg);
    assert!(Config::default().to_toml().is_ok());
}

#[test]
fn new_fields_have_the_documented_defaults() {
    let cfg = Config::default();
    assert_eq!(cfg.speech_margin_db, 10.0);
    assert_eq!(cfg.max_noise_floor_db, -40.0);
    assert_eq!(cfg.language, "auto");
    assert!(cfg.vad_model.is_none());
    assert_eq!(
        cfg.threads,
        std::thread::available_parallelism().unwrap().get()
    );
}

#[test]
fn new_fields_can_be_set_from_the_file() {
    let cfg = Config::parse(
        "speech_margin_db = 12.5\nmax_noise_floor_db = -42.0\nthreads = 6\nlanguage = \"ar\"\nvad_model = \"~/models/vad.bin\"\n",
    )
    .unwrap();
    assert_eq!(cfg.speech_margin_db, 12.5);
    assert_eq!(cfg.max_noise_floor_db, -42.0);
    assert_eq!(cfg.threads, 6);
    assert_eq!(cfg.language, "ar");
    assert_eq!(
        cfg.vad_model,
        Some(dirs::home_dir().unwrap().join("models/vad.bin"))
    );
    assert_eq!(Config::parse(&cfg.to_toml().unwrap()).unwrap(), cfg);
}

#[test]
fn explicit_missing_config_path_is_an_error() {
    assert!(Config::load(Some(std::path::Path::new("/nonexistent/config.toml"))).is_err());
}
