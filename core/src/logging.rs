use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chrono::{Local, SecondsFormat};
use regex::Regex;
use std::sync::OnceLock;

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(2);
static APPENDER: Mutex<()> = Mutex::new(());
static EMAIL_RE: OnceLock<Option<Regex>> = OnceLock::new();
static URL_RE: OnceLock<Option<Regex>> = OnceLock::new();
static POSIX_PATH_RE: OnceLock<Option<Regex>> = OnceLock::new();
static WINDOWS_PATH_RE: OnceLock<Option<Regex>> = OnceLock::new();

/// Returns the shared core and widget log folder.
pub fn log_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("MN_LOG_DIR") {
        return PathBuf::from(path);
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library").join("Logs").join("MeetingNotes")
    } else if cfg!(windows) {
        dirs::data_local_dir()
            .unwrap_or_else(|| home.join("AppData").join("Local"))
            .join("MeetingNotes")
            .join("logs")
    } else {
        home.join(".cache").join("meeting-notes")
    }
}

/// Writes an informational line to the shared log and stderr.
pub fn info(component: &str, msg: &str) {
    write_line("INFO", component, msg);
}

/// Writes a warning line to the shared log and stderr.
pub fn warn(component: &str, msg: &str) {
    write_line("WARN", component, msg);
}

/// Writes an error line to the shared log and stderr.
pub fn error(component: &str, msg: &str) {
    write_line("ERROR", component, msg);
}

/// Writes one process header with version and operating system details.
pub fn header(app: &str) {
    let os_version = os_version();
    info(
        app,
        &format!(
            "app={app} version={} os={} {} arch={}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            os_version,
            std::env::consts::ARCH
        ),
    );
}

/// Removes email addresses, URLs and the home path, then caps text at 500 characters.
pub fn scrub(text: &str) -> String {
    let mut value = text.to_string();
    if let Some(home) = dirs::home_dir().and_then(|p| p.to_str().map(str::to_string)) {
        if !home.is_empty() {
            value = value.replace(&home, "~");
            if cfg!(windows) {
                value = value.replace(&home.replace('/', "\\"), "~");
                value = value.replace(&home.replace('\\', "/"), "~");
            }
        }
    }
    if let Some(re) =
        EMAIL_RE.get_or_init(|| Regex::new(r"(?i)[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}").ok())
    {
        value = re.replace_all(&value, "<email>").into_owned();
    }
    if let Some(re) =
        URL_RE.get_or_init(|| Regex::new(r#"(?i)\b(?:https?://|ftp://|www\.)[^\s<>"']+"#).ok())
    {
        value = re.replace_all(&value, "<url>").into_owned();
    }
    if let Some(re) = POSIX_PATH_RE.get_or_init(|| Regex::new(r#"(^|[\s("'=:])(/[^\r\n]+)"#).ok()) {
        value = re.replace_all(&value, "$1<path>").into_owned();
    }
    if let Some(re) =
        WINDOWS_PATH_RE.get_or_init(|| Regex::new(r"(?i)([A-Z]:\\[^\r\n]+|\\\\[^\r\n]+)").ok())
    {
        value = re.replace_all(&value, "<path>").into_owned();
    }
    value.chars().take(500).collect()
}

pub(crate) fn session_name(path: &Path) -> String {
    let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
        return "session".to_string();
    };
    let bytes = name.as_bytes();
    let date_time = bytes.len() >= 15
        && bytes[0..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[7] == b'-'
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[10] == b'_'
        && bytes[11..15].iter().all(u8::is_ascii_digit);
    let suffix = bytes.get(15..).is_none_or(|tail| {
        tail.is_empty()
            || (tail.first() == Some(&b'-')
                && tail
                    .get(1..)
                    .is_some_and(|rest| !rest.is_empty() && rest.iter().all(u8::is_ascii_digit)))
    });
    if date_time && suffix {
        return name.to_string();
    }
    "session".to_string()
}

fn write_line(level: &str, component: &str, msg: &str) {
    let message = scrub(msg).replace(['\n', '\r'], " ");
    let line = format!(
        "{} {level:<5} [{component}] {}\n",
        Local::now().to_rfc3339_opts(SecondsFormat::Millis, false),
        message
    );
    eprint!("{line}");
    append_line(&log_dir(), line.as_bytes());
}

fn append_line(dir: &Path, line: &[u8]) {
    let _guard = APPENDER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !dir.is_dir() {
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let _ = crate::session::owner_only(dir);
    }
    let lock_path = dir.join("meeting-notes.log.lock");
    let _file_lock = acquire_file_lock(&lock_path);
    let current = dir.join("meeting-notes.log");
    let needs_rotation = fs::metadata(&current)
        .map(|metadata| metadata.len().saturating_add(line.len() as u64) > MAX_LOG_BYTES)
        .unwrap_or(false);
    if needs_rotation {
        rotate(dir);
    }
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(current) {
        let _ = file.write_all(line);
        let _ = file.flush();
    }
}

fn acquire_file_lock(path: &Path) -> Option<FileLock> {
    let started = Instant::now();
    loop {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(file) => {
                return Some(FileLock {
                    path: path.to_path_buf(),
                    file: Some(file),
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if stale_lock(path) {
                    let _ = fs::remove_file(path);
                }
                if started.elapsed() >= LOCK_WAIT {
                    return None;
                }
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return None,
        }
    }
}

struct FileLock {
    path: PathBuf,
    file: Option<std::fs::File>,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        self.file.take();
        let _ = fs::remove_file(&self.path);
    }
}

fn stale_lock(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .is_some_and(|age| age > Duration::from_secs(30))
}

fn rotate(dir: &Path) {
    let current = dir.join("meeting-notes.log");
    let first = dir.join("meeting-notes.log.1");
    let second = dir.join("meeting-notes.log.2");
    let _ = fs::remove_file(&second);
    let _ = fs::rename(&first, &second);
    let _ = fs::rename(current, first);
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
    std::process::Command::new(command.0)
        .args(command.1)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}
