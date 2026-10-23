//! Checking GitHub for a newer release, and the sentence the user pastes into Claude.
//! Nothing here touches Tauri.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// First check runs after startup, so a slow network does not delay the window.
pub const FIRST_CHECK: Duration = Duration::from_secs(30);
/// How often to look again after the first check.
pub const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

const LATEST_URL: &str =
    "https://api.github.com/repos/MustafaAFarag/Meeting-Minutes/releases/latest";

/// `latest` is newer than `current` as major.minor.patch.
///
/// A leading `v` on either side is ignored. Anything that is not three numbers is not newer.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

/// `v1.2.3` for a tag we could parse. `None` when `tag` is not major.minor.patch.
pub fn display_version(tag: &str) -> Option<String> {
    let (major, minor, patch) = parse_version(tag)?;
    Some(format!("v{major}.{minor}.{patch}"))
}

/// Three numeric components. A single leading `v` is allowed. Anything else is ignored.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim();
    let text = text.strip_prefix('v').unwrap_or(text);
    let mut parts = text.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?;
    if parts.next().is_some() || !patch.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((major, minor, patch.parse().ok()?))
}

/// The paste-into-Claude sentence. `steps` is the absolute path of `UPDATE.md`.
pub fn update_prompt(steps: &str) -> String {
    format!("Update Meeting Notes to the latest version by following the steps in {steps}")
}

/// `<home>/Meeting-Minutes/UPDATE.md` (backslashes on Windows).
pub fn update_steps_path(home: &Path) -> PathBuf {
    home.join("Meeting-Minutes").join("UPDATE.md")
}

/// The sentence for this machine's install clone.
pub fn update_prompt_for_home(home: &Path) -> String {
    update_prompt(&update_steps_path(home).display().to_string())
}

/// The `tag_name` string from a GitHub release JSON body.
pub fn tag_name(body: &str) -> Option<&str> {
    let key = "\"tag_name\"";
    let mut rest = body;
    while let Some(index) = rest.find(key) {
        let after = rest[index + key.len()..].trim_start();
        if let Some(after) = after.strip_prefix(':') {
            let after = after.trim_start();
            if let Some(value) = after.strip_prefix('"') {
                if let Some(end) = value.find('"') {
                    return Some(&value[..end]);
                }
            }
        }
        rest = &rest[index + key.len()..];
    }
    None
}

/// `fake` replaces the HTTP body, the way `MN_FAKE_LATEST` does at runtime.
///
/// When `fake` is set, `body` is not read. That is what lets a manual check skip the network:
/// `MN_FAKE_LATEST=v9.9.9`.
pub fn latest_tag_from(fake: Option<&str>, body: Result<&str, String>) -> Result<String, String> {
    if let Some(fake) = fake {
        return Ok(fake.to_string());
    }
    tag_name(body?)
        .map(str::to_string)
        .ok_or_else(|| "the release has no tag_name".to_string())
}

/// The latest release tag.
///
/// `MN_FAKE_LATEST`, when set, replaces the GitHub response (manual QA).
/// Example: `MN_FAKE_LATEST=v9.9.9`.
pub fn latest_tag() -> Result<String, String> {
    if let Ok(fake) = std::env::var("MN_FAKE_LATEST") {
        return latest_tag_from(Some(&fake), Err("skipped".into()));
    }
    let body = fetch_latest_body()?;
    latest_tag_from(None, Ok(&body))
}

fn fetch_latest_body() -> Result<String, String> {
    ureq::get(LATEST_URL)
        .set("User-Agent", "meeting-notes-app")
        .set("Accept", "application/vnd.github+json")
        .timeout(Duration::from_secs(10))
        .call()
        .map_err(|err| err.to_string())?
        .into_string()
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_patch_is_newer() {
        assert!(is_newer("0.1.5", "v0.1.6"));
    }

    #[test]
    fn equal_versions_are_not_newer() {
        assert!(!is_newer("0.1.5", "v0.1.5"));
        assert!(!is_newer("0.1.5", "0.1.5"));
    }

    #[test]
    fn an_older_release_is_not_newer() {
        assert!(!is_newer("0.1.5", "v0.1.4"));
        assert!(!is_newer("1.0.0", "v0.9.9"));
    }

    #[test]
    fn garbage_is_not_newer() {
        assert!(!is_newer("0.1.5", "garbage"));
        assert!(!is_newer("0.1.5", ""));
        assert!(!is_newer("nope", "v0.1.6"));
        assert!(!is_newer("0.1.5", "v1"));
        assert!(!is_newer("0.1.5", "v1.2.3-beta"));
    }

    #[test]
    fn numbers_compare_not_text() {
        assert!(is_newer("0.1.9", "v0.1.10"));
        assert!(!is_newer("0.1.10", "v0.1.9"));
        assert!(is_newer("0.1.5", "v0.2.0"));
        assert!(is_newer("0.1.5", "v1.0.0"));
    }

    #[test]
    fn display_version_keeps_three_numbers_and_a_v() {
        assert_eq!(display_version("v0.1.6").as_deref(), Some("v0.1.6"));
        assert_eq!(display_version("0.1.6").as_deref(), Some("v0.1.6"));
        assert_eq!(display_version("garbage"), None);
    }

    #[test]
    fn prompt_names_the_update_steps() {
        assert_eq!(
            update_prompt("/Users/jane/Meeting-Minutes/UPDATE.md"),
            "Update Meeting Notes to the latest version by following the steps in /Users/jane/Meeting-Minutes/UPDATE.md"
        );
    }

    #[test]
    fn prompt_keeps_a_windows_path_as_given() {
        assert_eq!(
            update_prompt(r"C:\Users\jane\Meeting-Minutes\UPDATE.md"),
            r"Update Meeting Notes to the latest version by following the steps in C:\Users\jane\Meeting-Minutes\UPDATE.md"
        );
    }

    #[test]
    fn steps_file_is_update_md_in_the_clone() {
        let steps = update_steps_path(Path::new("jane"));
        let parts: Vec<_> = steps
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(parts, ["jane", "Meeting-Minutes", "UPDATE.md"]);
    }

    #[test]
    fn tag_name_is_read_from_the_release_json() {
        let body = r#"{"url":"https://example.test","tag_name": "v0.1.6","name":"0.1.6"}"#;
        assert_eq!(tag_name(body), Some("v0.1.6"));
    }

    #[test]
    fn a_body_without_a_tag_is_none() {
        assert_eq!(tag_name("{}"), None);
        assert_eq!(tag_name("not json"), None);
        assert_eq!(tag_name(r#"{"tag_name": null}"#), None);
    }

    #[test]
    fn a_fake_tag_replaces_the_http_body() {
        let tag = latest_tag_from(Some("v9.9.9"), Err("offline".into())).unwrap();
        assert_eq!(tag, "v9.9.9");
        assert!(is_newer("0.1.5", &tag));
    }

    #[test]
    fn the_http_body_is_used_when_there_is_no_fake_tag() {
        let body = r#"{"tag_name":"v0.2.0"}"#;
        let tag = latest_tag_from(None, Ok(body)).unwrap();
        assert_eq!(tag, "v0.2.0");
    }
}
