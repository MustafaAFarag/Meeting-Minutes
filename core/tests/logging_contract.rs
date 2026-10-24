use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use mn_core::logging::{error, header, info, log_dir, scrub, warn};

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct LogDirGuard(Option<std::ffi::OsString>);

impl LogDirGuard {
    fn set(path: &std::path::Path) -> Self {
        let old = env::var_os("MN_LOG_DIR");
        env::set_var("MN_LOG_DIR", path);
        Self(old)
    }
}

impl Drop for LogDirGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => env::set_var("MN_LOG_DIR", value),
            None => env::remove_var("MN_LOG_DIR"),
        }
    }
}

#[test]
fn logger_writes_formatted_lines_and_a_process_header() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let _env = LogDirGuard::set(dir.path());

    header("mn");
    info("test", "record started");
    warn("test", "one track was silent");
    error("test", "helper exited with code 1");

    assert_eq!(log_dir(), dir.path());
    let log = fs::read_to_string(dir.path().join("meeting-notes.log")).unwrap();
    let lines: Vec<_> = log.lines().collect();
    assert_eq!(lines.len(), 4, "{log}");
    for line in &lines {
        assert!(
            regex::Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}[+-]\d{2}:\d{2} (INFO |WARN |ERROR) \[[^]]+\] .+$")
                .unwrap()
                .is_match(line),
            "bad log line: {line}"
        );
    }
    assert!(lines[0].contains("app=mn"), "{}", lines[0]);
    assert!(lines[0].contains(env!("CARGO_PKG_VERSION")), "{}", lines[0]);
    assert!(lines[0].contains(std::env::consts::OS), "{}", lines[0]);
    assert!(lines[0].contains(std::env::consts::ARCH), "{}", lines[0]);
    assert!(lines[1].contains("INFO  [test] record started"));
    assert!(lines[2].contains("WARN  [test] one track was silent"));
    assert!(lines[3].contains("ERROR [test] helper exited with code 1"));
}

#[test]
fn logger_rotates_at_one_megabyte_and_keeps_two_archives() {
    let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let _env = LogDirGuard::set(dir.path());
    fs::write(
        dir.path().join("meeting-notes.log"),
        vec![b'a'; 1024 * 1024],
    )
    .unwrap();
    fs::write(dir.path().join("meeting-notes.log.1"), "old one").unwrap();
    fs::write(dir.path().join("meeting-notes.log.2"), "old two").unwrap();

    info("test", "rotate");

    assert_eq!(
        fs::metadata(dir.path().join("meeting-notes.log.1"))
            .unwrap()
            .len(),
        1024 * 1024
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("meeting-notes.log.2")).unwrap(),
        "old one"
    );
    assert!(fs::read_to_string(dir.path().join("meeting-notes.log"))
        .unwrap()
        .contains("[test] rotate"));
    assert!(!PathBuf::from(dir.path().join("meeting-notes.log.2"))
        .as_os_str()
        .is_empty());
}

#[test]
fn scrub_removes_email_url_and_home_path_and_caps_output() {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/person"));
    let input = format!(
        "contact pat@example.test at https://example.test/path and {} {}",
        home.display(),
        "x".repeat(600)
    );

    let cleaned = scrub(&input);

    assert!(!cleaned.contains("pat@example.test"), "{cleaned}");
    assert!(!cleaned.contains("https://example.test"), "{cleaned}");
    assert!(
        !cleaned.contains(&home.to_string_lossy().to_string()),
        "{cleaned}"
    );
    assert!(cleaned.chars().count() <= 500);
}
