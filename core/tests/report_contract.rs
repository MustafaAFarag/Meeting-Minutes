use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

use mn_core::{create_report_in, Config, ReportDirs};
use zip::ZipArchive;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);

impl EnvGuard {
    fn set(values: &[(&'static str, PathBuf)]) -> Self {
        let previous = values
            .iter()
            .map(|(key, value)| {
                let old = env::var_os(key);
                env::set_var(key, value);
                (*key, old)
            })
            .collect();
        Self(previous)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, old) in self.0.drain(..) {
            match old {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }
}

#[test]
fn report_contains_only_diagnostics_and_redacts_config_paths() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let desktop = home.join("Desktop");
    let config_home = home.join(".config");
    let cache_home = home.join(".cache");
    let log_dir = root.path().join("logs");
    for dir in [&desktop, &config_home, &cache_home, &log_dir] {
        fs::create_dir_all(dir).unwrap();
    }
    let _env = EnvGuard::set(&[
        ("HOME", home.clone()),
        ("XDG_CONFIG_HOME", config_home.clone()),
        ("XDG_CACHE_HOME", cache_home.clone()),
        ("MN_LOG_DIR", log_dir.clone()),
    ]);
    fs::write(
        config_home.join("user-dirs.dirs"),
        "XDG_DESKTOP_DIR=\"$HOME/Desktop\"\n",
    )
    .unwrap();

    fs::write(log_dir.join("meeting-notes.log"), "current log\n").unwrap();
    fs::write(log_dir.join("meeting-notes.log.1"), "old log one\n").unwrap();
    fs::write(log_dir.join("meeting-notes.log.2"), "old log two\n").unwrap();
    let cache_dir = cache_home.join("meeting-notes");
    fs::create_dir_all(&cache_dir).unwrap();
    // An old widget.log from before the scrubber, with an e-mail and a link in it.
    fs::write(
        cache_dir.join("widget.log"),
        "old widget log jane.doe@example.com https://claude.ai/artifact/abc123\n",
    )
    .unwrap();
    let config_dir = config_home.join("meeting-notes");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("update.log"), "update log\n").unwrap();
    fs::write(config_dir.join("update-status.json"), "{\"state\":\"ok\"}").unwrap();

    let notes = home.join("MeetingNotes");
    let complete = notes.join("2026-10-24_1000");
    let unfinished = notes.join("2026-10-24_1100");
    fs::create_dir_all(&complete).unwrap();
    fs::create_dir_all(&unfinished).unwrap();
    fs::write(complete.join("session.json"), "{}").unwrap();
    fs::write(unfinished.join("session.json"), "{}").unwrap();
    fs::write(complete.join("transcript.md"), "PRIVATE MEETING TRANSCRIPT").unwrap();
    fs::write(complete.join("card.json"), "PRIVATE MEETING CARD").unwrap();
    fs::write(unfinished.join("audio.wav"), "PRIVATE AUDIO").unwrap();

    let cfg = Config {
        notes_dir: notes,
        my_name: Some("Pat Example".to_string()),
        artifact_url: None,
        vocab_prompt: Some("PRIVATE MEETING VOCABULARY".to_string()),
        ..Config::default()
    };
    fs::write(
        config_dir.join("artifact_url.txt"),
        "https://claude.ai/artifact/private-example\n",
    )
    .unwrap();
    let config_path = config_dir.join("config.toml");
    fs::write(&config_path, cfg.to_toml().unwrap()).unwrap();

    let folders = ReportDirs {
        output: desktop.clone(),
        config: config_home.clone(),
        cache: cache_home.clone(),
        logs: log_dir.clone(),
    };
    let report = create_report_in(&folders, Some(&config_path)).unwrap();
    assert!(report.starts_with(&desktop), "{}", report.display());
    assert!(report
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("MeetingNotes-report-"));

    let file = fs::File::open(report).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let names: Vec<_> = archive.file_names().map(str::to_string).collect();
    for expected in [
        "meeting-notes.log",
        "meeting-notes.log.1",
        "meeting-notes.log.2",
        "widget.log",
        "update.log",
        "update-status.json",
        "about.txt",
        "doctor.txt",
        "config.txt",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "{expected}: {names:?}"
        );
    }
    assert!(
        !names.iter().any(|name| name.contains("2026-10-24_")),
        "{names:?}"
    );

    let mut config = String::new();
    archive
        .by_name("config.txt")
        .unwrap()
        .read_to_string(&mut config)
        .unwrap();
    // The name and the page link identify the person: only whether they are set.
    assert!(!config.contains("Pat Example"), "{config}");
    assert!(!config.contains("private-example"), "{config}");
    assert!(config.contains("my_name = \"<set>\""), "{config}");
    assert!(config.contains("artifact_url = \"<set>\""), "{config}");
    assert!(config.contains("<redacted>"), "{config}");
    assert!(
        !config.contains(&home.to_string_lossy().to_string()),
        "{config}"
    );
    assert!(!config.contains("PRIVATE MEETING VOCABULARY"), "{config}");

    let mut about = String::new();
    archive
        .by_name("about.txt")
        .unwrap()
        .read_to_string(&mut about)
        .unwrap();
    assert!(about.contains("Notes sessions: 2"), "{about}");
    assert!(about.contains("Unfinished sessions: 1"), "{about}");
    let contents = names.join("\n");
    assert!(!contents.contains("PRIVATE MEETING"), "{contents}");
    let mut archive_contents = String::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut content = String::new();
        entry.read_to_string(&mut content).unwrap();
        archive_contents.push_str(&content);
    }
    assert!(
        !archive_contents.contains("PRIVATE MEETING"),
        "session contents leaked into report"
    );
    assert!(archive_contents.contains("old widget log"));
    assert!(!archive_contents.contains("jane.doe@example.com"));
    assert!(!archive_contents.contains("claude.ai/artifact/abc123"));
}

#[test]
fn report_config_lists_effective_fields_when_optional_values_are_unset() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let desktop = home.join("Desktop");
    let config_home = home.join(".config");
    let cache_home = home.join(".cache");
    let log_dir = root.path().join("logs");
    for dir in [&desktop, &config_home, &cache_home, &log_dir] {
        fs::create_dir_all(dir).unwrap();
    }
    let _env = EnvGuard::set(&[("HOME", home)]);
    // An empty config file: every optional value is unset.
    let config_dir = config_home.join("meeting-notes");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("config.toml");
    fs::write(&config_path, "").unwrap();
    let folders = ReportDirs {
        output: desktop,
        config: config_home,
        cache: cache_home,
        logs: log_dir,
    };

    let report = create_report_in(&folders, Some(&config_path)).unwrap();
    let file = fs::File::open(report).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut config = String::new();
    archive
        .by_name("config.txt")
        .unwrap()
        .read_to_string(&mut config)
        .unwrap();
    assert!(config.contains("my_name = \"<not set>\""), "{config}");
    assert!(config.contains("artifact_url = \"<not set>\""), "{config}");
    assert!(config.contains("notes_dir ="), "{config}");
}
