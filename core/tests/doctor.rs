//! The doctor command checks the local setup without recording or publishing anything.
#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mn_core::Config;
use serde_json::Value;
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    config: PathBuf,
    whisper: PathBuf,
    claude: PathBuf,
    model: PathBuf,
    vad: PathBuf,
    notes: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        common::use_test_log_dir();
        let dir = tempfile::tempdir().unwrap();
        let whisper = dir.path().join("fake-whisper");
        let claude = dir.path().join("fake-claude");
        let model = dir.path().join("ggml-small.bin");
        let vad = dir.path().join("ggml-silero.bin");
        let notes = dir.path().join("MeetingNotes");
        fs::create_dir_all(&notes).unwrap();
        fs::write(&model, vec![0; 1_100_000]).unwrap();
        fs::write(&vad, b"fake vad model").unwrap();
        Self {
            config: dir.path().join("config.toml"),
            _dir: dir,
            whisper,
            claude,
            model,
            vad,
            notes,
        }
    }

    fn script(path: &Path, body: &str) {
        fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    fn write_config(&self, with_model: bool, artifact_url: Option<&str>) {
        let mut cfg = Config {
            notes_dir: self.notes.clone(),
            whisper_cli: self.whisper.clone(),
            whisper_model: with_model.then(|| self.model.clone()),
            vad_model: Some(self.vad.clone()),
            claude_cli: self.claude.clone(),
            artifact_url: artifact_url.map(str::to_owned),
            ..Config::default()
        };
        // Keep this fixture's tests independent from a developer's installed command names.
        cfg.claude_config_dir = None;
        fs::write(&self.config, cfg.to_toml().unwrap()).unwrap();
    }

    fn run(&self, expected_version: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_mn"));
        command
            .arg("--config")
            .arg(&self.config)
            .arg("doctor")
            .env_remove("MN_EXPECTED_VERSION");
        if let Some(version) = expected_version {
            command.env("MN_EXPECTED_VERSION", version);
        }
        command.output().unwrap()
    }
}

fn doctor_result(output: &Output) -> Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().last().expect("doctor result line");
    serde_json::from_str(line.strip_prefix("DOCTOR_RESULT ").unwrap()).unwrap()
}

fn ready_fixture(claude_status: &str, artifact_url: Option<&str>) -> Fixture {
    let fixture = Fixture::new();
    Fixture::script(&fixture.whisper, "exit 0");
    Fixture::script(
        &fixture.claude,
        &format!("printf '%s\\n' '{claude_status}'\nexit 0"),
    );
    fixture.write_config(true, artifact_url);
    fixture
}

#[test]
fn reports_all_local_checks_ok() {
    let fixture = ready_fixture(
        r#"{"loggedIn":true}"#,
        Some("https://example.invalid/my-meetings"),
    );

    let output = fixture.run(None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result = doctor_result(&output);
    assert_eq!(result["ok"], true);
    assert_eq!(result["checks"].as_array().unwrap().len(), 8);
    assert!(String::from_utf8_lossy(&output.stdout)
        .contains("OK    whisper model found (ggml-small.bin, 1 MB)"));
}

#[test]
fn missing_whisper_model_fails() {
    let fixture = ready_fixture(
        r#"{"loggedIn":true}"#,
        Some("https://example.invalid/my-meetings"),
    );
    let mut cfg = Config::load(Some(&fixture.config)).unwrap();
    cfg.whisper_model = Some(fixture._dir.path().join("missing-model.bin"));
    fs::write(&fixture.config, cfg.to_toml().unwrap()).unwrap();

    let output = fixture.run(None);
    assert_eq!(output.status.code(), Some(1));
    let result = doctor_result(&output);
    assert_eq!(result["ok"], false);
    assert!(result["checks"].to_string().contains("model file"));
}

#[test]
fn claude_logged_out_fails_with_a_clear_next_step() {
    let fixture = ready_fixture(
        r#"{"loggedIn":false}"#,
        Some("https://example.invalid/my-meetings"),
    );

    let output = fixture.run(None);
    assert_eq!(output.status.code(), Some(1));
    let result = doctor_result(&output);
    assert_eq!(result["ok"], false);
    assert!(result["checks"].to_string().contains("claude auth login"));
}

#[test]
fn missing_artifact_url_warns_without_failing() {
    let fixture = ready_fixture(r#"{"loggedIn":true}"#, None);

    let output = fixture.run(None);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result = doctor_result(&output);
    assert_eq!(result["ok"], true);
    assert!(result["checks"].to_string().contains("warn"));
    assert!(result["checks"]
        .to_string()
        .contains("Your My Meetings page is not set up yet."));
}

#[test]
fn expected_version_mismatch_fails() {
    let fixture = ready_fixture(
        r#"{"loggedIn":true}"#,
        Some("https://example.invalid/my-meetings"),
    );

    let output = fixture.run(Some("v99.0.0"));
    assert_eq!(output.status.code(), Some(1));
    let result = doctor_result(&output);
    assert_eq!(result["ok"], false);
    assert!(result["checks"].to_string().contains("99.0.0"));
}
