use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub whisper_cli: PathBuf,
    pub whisper_model: Option<PathBuf>,
    /// Absolute level for a window to count as sound. "Them" (system audio) uses only this.
    pub silence_threshold_db: f64,
    /// "Me" (mic) only: a window is sound if it is at least this far above the mic's noise floor.
    pub speech_margin_db: f64,
    /// "Me" only: the noise-floor estimate is clamped to this, so speech can never become the floor.
    pub max_noise_floor_db: f64,
    pub silence_seconds: u64,
    pub vocab_prompt: Option<String>,
    /// whisper `-l`: a language code, or "auto".
    pub language: String,
    /// whisper `-t`.
    pub threads: usize,
    /// Silero VAD model for whisper's `--vad`. Off by default: it dropped real speech in testing.
    pub vad_model: Option<PathBuf>,
    /// argv of a command that writes raw s16le 16 kHz mono PCM to stdout (the mic).
    /// `None` means computed from `pactl get-default-source` at runtime.
    pub capture_me: Option<Vec<String>>,
    /// Same, for system audio. `None` means computed from `pactl get-default-sink` at runtime.
    pub capture_them: Option<Vec<String>>,
    /// The Claude Code CLI used for the card and the publish step.
    pub claude_cli: PathBuf,
    /// The Codex CLI, used when Claude fails to write the card.
    pub codex_cli: PathBuf,
    /// Exported as CLAUDE_CONFIG_DIR for every `claude` call (e.g. a work account's config dir).
    pub claude_config_dir: Option<PathBuf>,
    /// The PM's name, passed to the card prompt.
    pub my_name: Option<String>,
    /// The "My Meetings" artifact URL. `None` means read `artifact_url.txt` next to the config.
    pub artifact_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            notes_dir: home_dir().join("MeetingNotes"),
            whisper_cli: PathBuf::from("whisper-cli"),
            whisper_model: None,
            silence_threshold_db: -50.0,
            speech_margin_db: 10.0,
            max_noise_floor_db: -40.0,
            silence_seconds: 120,
            vocab_prompt: None,
            language: "auto".to_string(),
            threads: thread::available_parallelism().map_or(4, |n| n.get()),
            vad_model: None,
            capture_me: None,
            capture_them: None,
            claude_cli: PathBuf::from("claude"),
            codex_cli: PathBuf::from("codex"),
            claude_config_dir: None,
            my_name: None,
            artifact_url: None,
        }
    }
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let dir = dirs::config_dir().context("could not find the user config directory")?;
        Ok(dir.join("meeting-notes").join("config.toml"))
    }

    /// An explicit path must exist. The default path may be missing (all defaults).
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let path = match path {
            Some(p) => p.to_path_buf(),
            None => {
                let p = Self::default_path()?;
                if !p.exists() {
                    return Ok(Config::default());
                }
                p
            }
        };
        let text = fs::read_to_string(&path)
            .with_context(|| format!("reading config {}", path.display()))?;
        Self::parse(&text).with_context(|| format!("parsing config {}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Config> {
        let mut cfg: Config = toml::from_str(text)?;
        cfg.notes_dir = expand_tilde(&cfg.notes_dir);
        cfg.whisper_cli = expand_tilde(&cfg.whisper_cli);
        cfg.whisper_model = cfg.whisper_model.map(|p| expand_tilde(&p));
        cfg.vad_model = cfg.vad_model.map(|p| expand_tilde(&p));
        cfg.claude_cli = expand_tilde(&cfg.claude_cli);
        cfg.codex_cli = expand_tilde(&cfg.codex_cli);
        cfg.claude_config_dir = cfg.claude_config_dir.map(|p| expand_tilde(&p));
        Ok(cfg)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn capture_me_argv(&self) -> Result<Vec<String>> {
        match &self.capture_me {
            Some(argv) => Ok(argv.clone()),
            None => Ok(default_capture_me(&pactl_default(
                "get-default-source",
                "capture_me",
            )?)),
        }
    }

    pub fn capture_them_argv(&self) -> Result<Vec<String>> {
        match &self.capture_them {
            Some(argv) => Ok(argv.clone()),
            None => {
                let sink = pactl_default("get-default-sink", "capture_them")?;
                Ok(default_capture_them(&sink))
            }
        }
    }
}

pub fn default_capture_me(source: &str) -> Vec<String> {
    parec_argv(source)
}

pub fn default_capture_them(sink: &str) -> Vec<String> {
    parec_argv(&format!("{sink}.monitor"))
}

/// Always pass the device explicitly with `-d`, so the mic and the sink monitor stay separate tracks.
/// Without `--latency-msec` parec delivers audio in 2 s blocks, so every other second looks silent.
fn parec_argv(device: &str) -> Vec<String> {
    [
        "parec",
        "-d",
        device,
        "--format=s16le",
        "--rate=16000",
        "--channels=1",
        "--raw",
        "--latency-msec=100",
    ]
    .map(String::from)
    .to_vec()
}

/// Runs `pactl <query>` (get-default-source or get-default-sink) and returns the device name.
fn pactl_default(query: &str, config_key: &str) -> Result<String> {
    let out = Command::new("pactl").arg(query).output().with_context(|| {
        format!("could not run `pactl {query}`; set {config_key} in the config to skip it")
    })?;
    if !out.status.success() {
        bail!("`pactl {query}` failed with {}", out.status);
    }
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() {
        bail!("`pactl {query}` returned no device name");
    }
    Ok(name)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn expand_tilde(path: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home_dir().join(rest),
        Err(_) => path.to_path_buf(),
    }
}
