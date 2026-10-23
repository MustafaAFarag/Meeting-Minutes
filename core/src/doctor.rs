//! Local checks for an installed Meeting Notes setup.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::config::Config;
use crate::exec::{claude_command, resolve_claude_cli, resolves, run_with_timeout};
use crate::publish::known_url;
use crate::transcribe::whisper_command;

const DOCTOR_TIMEOUT: Duration = Duration::from_secs(120);
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(30);
const MIN_WHISPER_MODEL_BYTES: u64 = 1_000_000;
const WHISPER_SAMPLE_RATE: u32 = 16_000;
const WHISPER_SAMPLE_SECONDS: usize = 2;
static UNIQUE_PATH: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub version: String,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
}

impl DoctorReport {
    /// Formats the human checks followed by the machine-readable result line.
    pub fn render(&self) -> String {
        let mut lines = self
            .checks
            .iter()
            .map(|check| {
                let label = match check.status {
                    CheckStatus::Ok => "OK",
                    CheckStatus::Warn => "WARN",
                    CheckStatus::Fail => "FAIL",
                };
                let message = check.message.replace(['\n', '\r'], " ");
                format!("{label:<5} {message}")
            })
            .collect::<Vec<_>>();
        let json = serde_json::to_string(self)
            .unwrap_or_else(|_| "{\"ok\":false,\"version\":\"unknown\",\"checks\":[]}".to_string());
        lines.push(format!("DOCTOR_RESULT {json}"));
        format!("{}\n", lines.join("\n"))
    }
}

/// Loads the config and checks the local tools and folders without recording or publishing.
pub fn diagnose(config_arg: Option<&Path>, expected_version: Option<&str>) -> DoctorReport {
    let version = env!("CARGO_PKG_VERSION").to_string();
    let mut report = DoctorReport {
        ok: true,
        version: version.clone(),
        checks: Vec::new(),
    };

    let config_path = match config_arg
        .map(Path::to_path_buf)
        .map(Ok)
        .unwrap_or_else(Config::default_path)
    {
        Ok(path) => path,
        Err(error) => {
            report.checks.push(check(
                "config",
                CheckStatus::Fail,
                format!(
                    "Could not find your settings file location: {error:#}. Fix your user settings and run `mn doctor` again."
                ),
            ));
            report
                .checks
                .push(version_check(&version, expected_version));
            report.ok = report
                .checks
                .iter()
                .all(|item| !matches!(item.status, CheckStatus::Fail));
            return report;
        }
    };

    let cfg = match Config::load(config_arg) {
        Ok(cfg) => {
            let message = if config_arg.is_none() && !config_path.exists() {
                format!(
                    "Using the default settings because no config file exists at {}.",
                    config_path.display()
                )
            } else {
                format!("Settings loaded from {}.", config_path.display())
            };
            report
                .checks
                .push(check("config", CheckStatus::Ok, message));
            cfg
        }
        Err(error) => {
            report.checks.push(check(
                "config",
                CheckStatus::Fail,
                format!(
                    "Could not load your settings from {}: {error:#}. Fix that file and run `mn doctor` again.",
                    config_path.display()
                ),
            ));
            report
                .checks
                .push(version_check(&version, expected_version));
            report.ok = report
                .checks
                .iter()
                .all(|item| !matches!(item.status, CheckStatus::Fail));
            return report;
        }
    };

    report
        .checks
        .push(version_check(&version, expected_version));

    let (whisper_check, whisper_ready) = check_whisper(&cfg);
    report.checks.push(whisper_check);
    report.checks.push(check_vad(&cfg));
    if whisper_ready {
        report.checks.push(check_whisper_runs(&cfg));
    } else {
        report.checks.push(check(
            "whisper_run",
            CheckStatus::Warn,
            "Whisper's test was skipped because its command or model is not ready.".to_string(),
        ));
    }
    report.checks.push(check_claude(&cfg));
    report.checks.push(check_notes_dir(&cfg));
    report.checks.push(check_page(&cfg, &config_path));
    report.ok = report
        .checks
        .iter()
        .all(|item| !matches!(item.status, CheckStatus::Fail));
    report
}

fn version_check(version: &str, expected: Option<&str>) -> DoctorCheck {
    match expected.map(|value| value.strip_prefix('v').unwrap_or(value)) {
        Some(expected) if expected != version => check(
            "version",
            CheckStatus::Fail,
            format!(
                "This is Meeting Notes {version}, but the update expected {expected}. Run the update again and keep its version."
            ),
        ),
        Some(_) => check(
            "version",
            CheckStatus::Ok,
            format!("Meeting Notes version {version} matches the update."),
        ),
        None => check(
            "version",
            CheckStatus::Ok,
            format!("Meeting Notes version {version} is installed."),
        ),
    }
}

fn check_whisper(cfg: &Config) -> (DoctorCheck, bool) {
    if !resolves(&cfg.whisper_cli) {
        return (
            check(
                "whisper",
                CheckStatus::Fail,
                format!(
                    "Whisper could not be found at {}; run the Meeting Notes setup again.",
                    cfg.whisper_cli.display()
                ),
            ),
            false,
        );
    }
    let Some(model) = cfg.whisper_model.as_deref() else {
        return (
            check(
                "whisper",
                CheckStatus::Fail,
                "No Whisper model is set in your settings; run the Meeting Notes setup again."
                    .to_string(),
            ),
            false,
        );
    };
    let metadata = match fs::metadata(model) {
        Ok(metadata) if metadata.is_file() && metadata.len() > MIN_WHISPER_MODEL_BYTES => metadata,
        Ok(_) => {
            return (
                check(
                    "whisper",
                    CheckStatus::Fail,
                    format!(
                        "The Whisper model at {} is too small or is not a file; run the Meeting Notes setup again.",
                        model.display()
                    ),
                ),
                false,
            );
        }
        Err(_) => {
            return (
                check(
                    "whisper",
                    CheckStatus::Fail,
                    format!(
                        "The Whisper model file {} is missing; run the Meeting Notes setup again.",
                        model.display()
                    ),
                ),
                false,
            );
        }
    };
    let name = model
        .file_name()
        .map(|part| part.to_string_lossy().into_owned())
        .unwrap_or_else(|| model.display().to_string());
    let size_mb = (metadata.len() + 500_000) / 1_000_000;
    (
        check(
            "whisper",
            CheckStatus::Ok,
            format!("whisper model found ({name}, {size_mb} MB); whisper is ready."),
        ),
        true,
    )
}

fn check_vad(cfg: &Config) -> DoctorCheck {
    match cfg.vad_model.as_deref() {
        Some(path) if path.is_file() => check(
            "vad",
            CheckStatus::Ok,
            "Whisper's speech detection file is ready.".to_string(),
        ),
        Some(_) => check(
            "vad",
            CheckStatus::Warn,
            "Whisper's speech detection file is missing; run the Meeting Notes setup again to download it."
                .to_string(),
        ),
        None => check(
            "vad",
            CheckStatus::Warn,
            "Whisper's speech detection file is not set; run the Meeting Notes setup again to download it."
                .to_string(),
        ),
    }
}

fn check_whisper_runs(cfg: &Config) -> DoctorCheck {
    match run_whisper_check(cfg) {
        Ok(()) => check(
            "whisper_run",
            CheckStatus::Ok,
            "Whisper processed a short silence test.".to_string(),
        ),
        Err(error) => check(
            "whisper_run",
            CheckStatus::Fail,
            format!("Whisper could not complete its short test: {error}. Run the Meeting Notes setup again."),
        ),
    }
}

fn run_whisper_check(cfg: &Config) -> Result<()> {
    let model = cfg
        .whisper_model
        .as_deref()
        .context("the model is not set")?;
    let temp = DoctorTempDir::create()?;
    let wav = temp.path.join("silence.wav");
    write_silence_wav(&wav)?;
    let mut cmd = whisper_command(cfg, model, &wav, &temp.path.join("doctor"));
    match run_with_timeout(&mut cmd, DOCTOR_TIMEOUT).context("could not start Whisper")? {
        Some(output) if output.status.success() => Ok(()),
        Some(_) => anyhow::bail!("Whisper did not accept the test audio"),
        None => anyhow::bail!("Whisper took longer than two minutes"),
    }
}

fn write_silence_wav(path: &Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: WHISPER_SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .with_context(|| format!("creating the short test file at {}", path.display()))?;
    for _ in 0..WHISPER_SAMPLE_RATE as usize * WHISPER_SAMPLE_SECONDS {
        writer.write_sample(0_i16)?;
    }
    writer.finalize()?;
    Ok(())
}

fn check_claude(cfg: &Config) -> DoctorCheck {
    let cli = resolve_claude_cli(&cfg.claude_cli);
    if !resolves(&cli) {
        return check(
            "claude",
            CheckStatus::Fail,
            format!(
                "Claude could not be found at {}; run the Meeting Notes setup again.",
                cli.display()
            ),
        );
    }
    let mut cmd = claude_command(cfg);
    cmd.args(["auth", "status", "--json"]);
    match run_with_timeout(&mut cmd, CLAUDE_TIMEOUT) {
        Ok(Some(output)) if output.status.success() && !auth_reports_logged_out(&output.stdout) => {
            check(
                "claude",
                CheckStatus::Ok,
                "Claude is signed in and ready.".to_string(),
            )
        }
        _ => check(
            "claude",
            CheckStatus::Fail,
            "Claude is not signed in. Ask Claude to run `claude auth login` and click Authorize in the browser."
                .to_string(),
        ),
    }
}

fn auth_reports_logged_out(stdout: &[u8]) -> bool {
    let text = String::from_utf8_lossy(stdout);
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
        if let Some(logged_in) = find_login_flag(&value) {
            return !logged_in;
        }
    }
    let lowercase = text.to_lowercase();
    [
        "not logged in",
        "logged out",
        "signed out",
        "not authenticated",
    ]
    .iter()
    .any(|phrase| lowercase.contains(phrase))
}

fn find_login_flag(value: &serde_json::Value) -> Option<bool> {
    match value {
        serde_json::Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(key.as_str(), "loggedIn" | "logged_in") {
                    if let Some(flag) = value.as_bool() {
                        return Some(flag);
                    }
                }
                if let Some(flag) = find_login_flag(value) {
                    return Some(flag);
                }
            }
            None
        }
        serde_json::Value::Array(values) => values.iter().find_map(find_login_flag),
        _ => None,
    }
}

fn check_notes_dir(cfg: &Config) -> DoctorCheck {
    match writable_directory(&cfg.notes_dir) {
        Ok(()) => check(
            "notes",
            CheckStatus::Ok,
            "Your notes folder exists and can save files.".to_string(),
        ),
        Err(_) => check(
            "notes",
            CheckStatus::Fail,
            format!(
                "Your notes folder at {} cannot save files; choose a folder you can write to and run setup again.",
                cfg.notes_dir.display()
            ),
        ),
    }
}

fn writable_directory(path: &Path) -> Result<()> {
    if !path.is_dir() {
        anyhow::bail!("folder does not exist");
    }
    let probe = path.join(format!(
        ".mn-doctor-{}-{}",
        std::process::id(),
        UNIQUE_PATH.fetch_add(1, Ordering::Relaxed)
    ));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .with_context(|| format!("creating {}", probe.display()))?;
    fs::remove_file(&probe).with_context(|| format!("removing {}", probe.display()))?;
    Ok(())
}

fn check_page(cfg: &Config, config_path: &Path) -> DoctorCheck {
    let url_file = config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("artifact_url.txt");
    if known_url(cfg, &url_file).is_some() {
        check(
            "page",
            CheckStatus::Ok,
            "Your My Meetings page is set up.".to_string(),
        )
    } else {
        check(
            "page",
            CheckStatus::Warn,
            "Your My Meetings page is not set up yet.".to_string(),
        )
    }
}

fn check(name: &str, status: CheckStatus, message: String) -> DoctorCheck {
    DoctorCheck {
        name: name.to_string(),
        status,
        message,
    }
}

struct DoctorTempDir {
    path: PathBuf,
}

impl DoctorTempDir {
    fn create() -> Result<Self> {
        let root = std::env::temp_dir();
        for _ in 0..10 {
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = root.join(format!(
                "meeting-notes-doctor-{}-{stamp}-{}",
                std::process::id(),
                UNIQUE_PATH.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(error).with_context(|| format!("creating {}", path.display()));
                }
            }
        }
        anyhow::bail!("could not make a temporary folder for the Whisper check")
    }
}

impl Drop for DoctorTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
