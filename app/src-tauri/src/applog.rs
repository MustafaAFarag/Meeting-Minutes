//! The widget's lines in the shared log (`meeting-notes.log`). Every line goes through the core
//! logger, tagged `[widget]`, and through `scrub` first, so no e-mail, link or home folder path
//! gets in. Meeting content never reaches these functions: callers log stages, counts and
//! session folder names only.

use std::any::Any;

use mn_core::logging;
use mn_core::Failure;

const COMPONENT: &str = "widget";

pub fn info(msg: &str) {
    logging::info(COMPONENT, &logging::scrub(msg));
}

pub fn warn(msg: &str) {
    logging::warn(COMPONENT, &logging::scrub(msg));
}

pub fn error(msg: &str) {
    logging::error(COMPONENT, &logging::scrub(msg));
}

/// The technical reason for a failure, for the log only. The core already scrubbed the detail it
/// attached; anything else is scrubbed here.
pub fn failure_detail(err: &anyhow::Error) -> String {
    match err.downcast_ref::<Failure>() {
        Some(failure) => failure.detail.clone(),
        None => logging::scrub(&format!("{err:#}")),
    }
}

/// The last two parts of a source path ("src/lib.rs"), so no folder of the build machine is logged.
fn short_path(file: &str) -> String {
    let parts: Vec<&str> = file.split(['/', '\\']).collect();
    let from = parts.len().saturating_sub(2);
    parts[from..].join("/")
}

/// One log line for a panic: what it said, which thread, and where.
pub fn panic_line(
    payload: &(dyn Any + Send),
    location: Option<(&str, u32)>,
    thread: &str,
) -> String {
    let text = payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".to_string());
    let at = location
        .map(|(file, line)| format!(" at {}:{line}", short_path(file)))
        .unwrap_or_default();
    format!("panic: {text} [thread '{thread}'{at}]")
}

/// Logs every panic (payload and location). The threads that catch panics keep doing so; this
/// only makes sure the reason is not lost.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|at| (at.file(), at.line()));
        let current = std::thread::current();
        let line = panic_line(
            info.payload(),
            location,
            current.name().unwrap_or("unnamed"),
        );
        error(&line);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use mn_core::{ErrorKind, Stage};

    #[test]
    fn a_panic_line_has_the_message_the_thread_and_a_short_place() {
        let line = panic_line(
            &"boom",
            Some(("/home/someone/app/src/lib.rs", 42)),
            "mn-run",
        );
        assert_eq!(line, "panic: boom [thread 'mn-run' at src/lib.rs:42]");
        let owned = panic_line(&String::from("bad value"), None, "main");
        assert_eq!(owned, "panic: bad value [thread 'main']");
        let other = panic_line(&42_u32, Some(("C:\\build\\src\\queue.rs", 7)), "t");
        assert_eq!(other, "panic: (no message) [thread 't' at src/queue.rs:7]");
    }

    #[test]
    fn the_failure_detail_is_what_the_core_attached() {
        let err = Failure::attach(
            anyhow!("claude exited 1"),
            ErrorKind::ClaudeFailed,
            Stage::Summarize,
        );
        assert!(failure_detail(&err).contains("claude exited 1"));
        let plain = anyhow!("mail me at a.person@example.org");
        let shown = failure_detail(&plain);
        assert!(!shown.contains("a.person@example.org"), "{shown}");
        assert!(shown.contains("<email>"));
    }

    #[test]
    fn lines_reach_the_shared_log_tagged_and_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        // The only test in this crate that logs, so the process-wide variable is not contended.
        std::env::set_var("MN_LOG_DIR", dir.path());
        info("queue take session=2026-10-24_1405");
        warn("could not reach a.person@example.org at https://example.org/x");
        std::env::remove_var("MN_LOG_DIR");
        let text = std::fs::read_to_string(dir.path().join("meeting-notes.log")).unwrap();
        assert!(
            text.contains("INFO  [widget] queue take session=2026-10-24_1405"),
            "{text}"
        );
        assert!(
            text.contains("WARN  [widget] could not reach <email> at <url>"),
            "{text}"
        );
        assert!(!text.contains("example.org"), "{text}");
    }
}
