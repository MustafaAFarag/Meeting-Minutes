use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use chrono::Local;
use zip::write::{SimpleFileOptions, ZipWriter};
use zip::CompressionMethod;

use crate::config::Config;
use crate::doctor::diagnose;
use crate::logging::{log_dir, session_name};
use crate::publish::known_url;

/// The folders a report reads from and writes to. Tests pass temp folders, because macOS and
/// Windows ignore the XDG variables that would redirect `dirs` on Linux.
pub struct ReportDirs {
    /// Where the zip goes (the Desktop, or the home folder when there is none).
    pub output: PathBuf,
    /// The OS config folder (holds `meeting-notes/`).
    pub config: PathBuf,
    /// The OS cache folder (holds `meeting-notes/widget.log`).
    pub cache: PathBuf,
    /// The shared log folder.
    pub logs: PathBuf,
}

impl ReportDirs {
    /// This computer's folders.
    pub fn for_this_machine() -> Result<Self> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let output = dirs::desktop_dir()
            .filter(|path| fs::create_dir_all(path).is_ok())
            .or_else(|| Some(home.clone()).filter(|path| fs::create_dir_all(path).is_ok()))
            .context("could not create a report folder on the Desktop or in the home folder")?;
        Ok(ReportDirs {
            output,
            config: dirs::config_dir().unwrap_or_else(|| home.join(".config")),
            cache: dirs::cache_dir().unwrap_or_else(|| home.join(".cache")),
            logs: log_dir(),
        })
    }
}

/// Creates a diagnostics archive on the Desktop, falling back to the home folder.
pub fn create_report(config_path: Option<&Path>) -> Result<PathBuf> {
    create_report_in(&ReportDirs::for_this_machine()?, config_path)
}

/// Creates the archive in `folders.output`, reading logs and settings from `folders`.
pub fn create_report_in(folders: &ReportDirs, config_path: Option<&Path>) -> Result<PathBuf> {
    let name = format!(
        "MeetingNotes-report-{}.zip",
        Local::now().format("%Y-%m-%d_%H%M")
    );
    let output = folders.output.join(name);
    create_report_at(config_path, &output, folders)?;
    Ok(output)
}

fn create_report_at(config_path: Option<&Path>, output: &Path, folders: &ReportDirs) -> Result<()> {
    let log_folder = &folders.logs;
    let mut cfg = Config::load(config_path).unwrap_or_default();
    let config_path = config_path
        .map(Path::to_path_buf)
        .or_else(|| Config::default_path().ok());
    let config_dir = folders.config.clone();
    let url_file = config_dir.join("meeting-notes").join("artifact_url.txt");
    if cfg.artifact_url.is_none() {
        cfg.artifact_url = known_url(&cfg, &url_file);
    }
    let notes_dir = cfg.notes_dir.clone();
    let file = File::create(output).with_context(|| format!("creating {}", output.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    for name in [
        "meeting-notes.log",
        "meeting-notes.log.1",
        "meeting-notes.log.2",
    ] {
        add_log_if_present(&mut zip, options, log_folder.join(name), name)?;
    }
    let cache_dir = folders.cache.clone();
    add_log_if_present(
        &mut zip,
        options,
        cache_dir.join("meeting-notes").join("widget.log"),
        "widget.log",
    )?;

    let update_dir = config_dir.join("meeting-notes");
    add_log_if_present(
        &mut zip,
        options,
        update_dir.join("update.log"),
        "update.log",
    )?;
    add_log_if_present(
        &mut zip,
        options,
        update_dir.join("update-status.json"),
        "update-status.json",
    )?;

    let about = about_text(&notes_dir);
    add_bytes(&mut zip, options, "about.txt", about.as_bytes())?;
    let expected_version = std::env::var("MN_EXPECTED_VERSION").ok();
    let doctor =
        redact_home_path(&diagnose(config_path.as_deref(), expected_version.as_deref()).render());
    add_bytes(&mut zip, options, "doctor.txt", doctor.as_bytes())?;
    let config_text = cfg
        .to_toml()
        .ok()
        .map(|text| redact_config(text, &cfg))
        .unwrap_or_else(|| "Config could not be loaded. Run mn doctor for details.\n".to_string());
    add_bytes(&mut zip, options, "config.txt", config_text.as_bytes())?;

    zip.finish().context("finishing report archive")?;
    Ok(())
}

/// Adds a log through `scrub()`, line by line: older logs (widget.log before v0.1.10, update.log
/// with setup output) were written without it and can hold e-mails, links or home paths.
fn add_log_if_present(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    source: PathBuf,
    name: &str,
) -> Result<()> {
    let bytes = match fs::read(&source) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", source.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut clean = String::with_capacity(text.len());
    for line in text.lines() {
        clean.push_str(&crate::logging::scrub(line));
        clean.push('\n');
    }
    add_bytes(zip, options, name, clean.as_bytes())
}

fn add_bytes(
    zip: &mut ZipWriter<File>,
    options: SimpleFileOptions,
    name: &str,
    bytes: &[u8],
) -> Result<()> {
    zip.start_file(name, options)
        .with_context(|| format!("adding {name} to report"))?;
    zip.write_all(bytes)
        .with_context(|| format!("writing {name} to report"))
}

fn about_text(notes_dir: &Path) -> String {
    let (sessions, unfinished) = count_sessions(notes_dir);
    let install_dir_exists = std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::exists))
        .unwrap_or(false);
    let tag = meeting_repo_tag();
    format!(
        "Meeting Notes version: {}\nmn version: {}\nOS: {} {}\nArchitecture: {}\nInstall folder exists: {}\nMeeting-Minutes git tag: {}\nNotes sessions: {}\nUnfinished sessions: {}\n",
        env!("CARGO_PKG_VERSION"),
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        os_version(),
        std::env::consts::ARCH,
        install_dir_exists,
        tag,
        sessions,
        unfinished,
    )
}

fn count_sessions(notes_dir: &Path) -> (usize, usize) {
    let Ok(entries) = fs::read_dir(notes_dir) else {
        return (0, 0);
    };
    let mut sessions = 0;
    let mut unfinished = 0;
    for entry in entries.flatten() {
        let dir = entry.path();
        let Some(name) = dir.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with('.')
            || !dir.is_dir()
            || (!dir.join("session.json").is_file() && session_name(&dir) == "session")
        {
            continue;
        }
        sessions += 1;
        if !dir.join("transcript.md").is_file()
            || !dir.join("card.json").is_file()
            || dir.join("PUBLISH_PENDING").exists()
        {
            unfinished += 1;
        }
    }
    (sessions, unfinished)
}

fn redact_config(text: String, cfg: &Config) -> String {
    let Ok(mut value) = toml::from_str::<toml::Value>(&text) else {
        return "Config could not be safely rendered. Run mn doctor for details.\n".to_string();
    };
    if let Some(table) = value.as_table_mut() {
        let home_user = dirs::home_dir().and_then(|home| {
            home.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        });
        let display_name = cfg.my_name.as_deref();
        for key in [
            "notes_dir",
            "whisper_cli",
            "whisper_model",
            "vad_model",
            "claude_cli",
            "codex_cli",
            "claude_config_dir",
        ] {
            if let Some(toml::Value::String(path)) = table.get_mut(key) {
                if contains_user_path(path, home_user.as_deref(), display_name) {
                    *path = "<redacted>".to_string();
                }
            }
        }
        if let Some(toml::Value::Array(values)) = table.get_mut("capture_me") {
            redact_path_array(values, home_user.as_deref(), display_name);
        }
        if let Some(toml::Value::Array(values)) = table.get_mut("capture_them") {
            redact_path_array(values, home_user.as_deref(), display_name);
        }
        if cfg.vocab_prompt.is_some() {
            table.insert(
                "vocab_prompt".to_string(),
                toml::Value::String("<redacted>".to_string()),
            );
        }
        // The page link, the name and the notes folder identify the person: say only whether they are set.
        let presence =
            |set: bool| toml::Value::String(if set { "<set>" } else { "<not set>" }.to_string());
        let name_set = cfg.my_name.as_deref().is_some_and(|n| !n.trim().is_empty());
        let url_set = cfg
            .artifact_url
            .as_deref()
            .is_some_and(|u| !u.trim().is_empty());
        table.insert("my_name".to_string(), presence(name_set));
        table.insert("artifact_url".to_string(), presence(url_set));
        table.insert("notes_dir".to_string(), presence(true));
    }
    toml::to_string_pretty(&value).unwrap_or_else(|_| {
        "Config could not be safely rendered. Run mn doctor for details.\n".to_string()
    })
}

fn redact_path_array(
    values: &mut [toml::Value],
    home_user: Option<&str>,
    display_name: Option<&str>,
) {
    for value in values {
        if let toml::Value::String(path) = value {
            if contains_user_path(path, home_user, display_name) {
                *path = "<redacted>".to_string();
            }
        }
    }
}

fn contains_user_path(path: &str, home_user: Option<&str>, display_name: Option<&str>) -> bool {
    let path = path.to_lowercase();
    let contains_account = home_user
        .filter(|user| !user.is_empty())
        .is_some_and(|user| {
            path.split(['/', '\\'])
                .any(|part| part == user.to_lowercase())
        });
    let contains_display_name = display_name
        .filter(|name| !name.trim().is_empty())
        .is_some_and(|name| path.contains(&name.to_lowercase()));
    contains_account || contains_display_name
}

fn redact_home_path(text: &str) -> String {
    let Some(home) = dirs::home_dir().and_then(|path| path.to_str().map(str::to_string)) else {
        return text.to_string();
    };
    let mut redacted = text.replace(&home, "~");
    if cfg!(windows) {
        redacted = redacted.replace(&home.replace('/', "\\"), "~");
        redacted = redacted.replace(&home.replace('\\', "/"), "~");
    }
    redacted
}

fn meeting_repo_tag() -> String {
    let Some(home) = dirs::home_dir() else {
        return "not found".to_string();
    };
    let output = Command::new("git")
        .arg("-C")
        .arg(home.join("Meeting-Minutes"))
        .args(["describe", "--tags", "--abbrev=0"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|tag| !tag.is_empty())
        .unwrap_or_else(|| "not found".to_string())
}

fn os_version() -> String {
    #[cfg(target_os = "macos")]
    let command = ("sw_vers", vec!["-productVersion"]);
    #[cfg(target_os = "windows")]
    let command = ("cmd", vec!["/C", "ver"]);
    #[cfg(all(unix, not(target_os = "macos")))]
    let command = ("uname", vec!["-r"]);
    #[cfg(not(any(unix, windows)))]
    let command = ("", Vec::<&str>::new());

    if command.0.is_empty() {
        return "unknown".to_string();
    }
    Command::new(command.0)
        .args(command.1)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
