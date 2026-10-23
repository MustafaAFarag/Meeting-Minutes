use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(any(windows, target_os = "macos")))]
use std::process::Command;
use std::thread;

#[cfg(not(any(windows, target_os = "macos")))]
use anyhow::bail;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::capture::Track;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub notes_dir: PathBuf,
    pub whisper_cli: PathBuf,
    pub whisper_model: Option<PathBuf>,
    /// Keys of the removed silence auto-stop. Old config files may still set them: they are
    /// accepted, ignored, and never printed. Any other unknown key is still an error.
    #[serde(default, skip_serializing)]
    pub silence_threshold_db: Option<toml::Value>,
    #[serde(default, skip_serializing)]
    pub speech_margin_db: Option<toml::Value>,
    #[serde(default, skip_serializing)]
    pub max_noise_floor_db: Option<toml::Value>,
    #[serde(default, skip_serializing)]
    pub silence_seconds: Option<toml::Value>,
    pub vocab_prompt: Option<String>,
    /// whisper `-l`: a language code, or "auto".
    pub language: String,
    /// whisper `-t`.
    pub threads: usize,
    /// Silero VAD model for whisper's `--vad` (the setup scripts download it). Unset means no VAD:
    /// whisper then also transcribes the silences, where it can invent lines.
    pub vad_model: Option<PathBuf>,
    /// argv of a command that writes raw s16le 16 kHz mono PCM to stdout (the mic).
    /// `None` means the per-OS default: Linux computes it from `pactl get-default-source`;
    /// Windows and macOS run this executable as `capture mic`.
    pub capture_me: Option<Vec<String>>,
    /// Same, for system audio. `None` means the per-OS default: Linux computes it from
    /// `pactl get-default-sink`; Windows runs `capture system`; macOS runs the `mn-audiotap`
    /// helper next to this executable.
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
            whisper_cli: PathBuf::from(if cfg!(windows) {
                "whisper-cli.exe"
            } else {
                "whisper-cli"
            }),
            whisper_model: None,
            silence_threshold_db: None,
            speech_margin_db: None,
            max_noise_floor_db: None,
            silence_seconds: None,
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
        cfg.silence_threshold_db = None;
        cfg.speech_margin_db = None;
        cfg.max_noise_floor_db = None;
        cfg.silence_seconds = None;
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
            None => platform_capture_me(),
        }
    }

    pub fn capture_them_argv(&self) -> Result<Vec<String>> {
        match &self.capture_them {
            Some(argv) => Ok(argv.clone()),
            None => platform_capture_them(),
        }
    }

    /// The comment `mn config` prints for a capture command that is not set, `None` when it
    /// is set. Linux only names the pactl query (running it here would make `mn config` fail
    /// without PulseAudio); Windows and macOS print the computed command.
    pub fn capture_comment(&self, track: Track) -> Option<String> {
        let (key, set, linux_query) = match track {
            Track::Me => ("capture_me", &self.capture_me, "get-default-source"),
            Track::Them => ("capture_them", &self.capture_them, "get-default-sink"),
        };
        if set.is_some() {
            return None;
        }
        if cfg!(any(windows, target_os = "macos")) {
            let argv = match track {
                Track::Me => self.capture_me_argv(),
                Track::Them => self.capture_them_argv(),
            };
            return Some(match argv {
                Ok(argv) => format!("# {key}: default is {argv:?}"),
                Err(err) => format!("# {key}: no default ({err:#})"),
            });
        }
        Some(format!(
            "# {key}: computed at runtime from `pactl {linux_query}`"
        ))
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_capture_me() -> Result<Vec<String>> {
    Ok(default_capture_me(&pactl_default(
        "get-default-source",
        "capture_me",
    )?))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn platform_capture_them() -> Result<Vec<String>> {
    let sink = pactl_default("get-default-sink", "capture_them")?;
    Ok(default_capture_them(&sink))
}

#[cfg(any(windows, target_os = "macos"))]
fn platform_capture_me() -> Result<Vec<String>> {
    Ok(native_capture_argv(&current_exe()?, "mic"))
}

#[cfg(windows)]
fn platform_capture_them() -> Result<Vec<String>> {
    Ok(native_capture_argv(&current_exe()?, "system"))
}

#[cfg(target_os = "macos")]
fn platform_capture_them() -> Result<Vec<String>> {
    Ok(audiotap_argv(&current_exe()?))
}

#[cfg(any(windows, target_os = "macos"))]
fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("could not find this executable to start the capture command")
}

/// `<exe> capture <source>`: this program's own built-in capture, so the same default works
/// from `mn` and from the widget (which has no `mn` next to it).
pub fn native_capture_argv(exe: &Path, source: &str) -> Vec<String> {
    vec![
        exe.to_string_lossy().into_owned(),
        "capture".to_string(),
        source.to_string(),
    ]
}

/// The macOS system-audio helper, bundled next to the app executable.
pub fn audiotap_argv(exe: &Path) -> Vec<String> {
    let dir = exe.parent().unwrap_or(Path::new("."));
    vec![dir.join("mn-audiotap").to_string_lossy().into_owned()]
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
#[cfg(not(any(windows, target_os = "macos")))]
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
