//! One-click update: the status file, when to refuse, and how the updater script is started.
//! The script itself lives in the install clone. Nothing here touches Tauri.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often to read the status file while an update is running.
pub const POLL_EVERY: Duration = Duration::from_secs(2);
/// Give up waiting after this long. The setup script quits the app sooner.
pub const POLL_FOR: Duration = Duration::from_secs(15 * 60);
/// A "running" status older than this did not finish.
pub const STALE_RUNNING: Duration = Duration::from_secs(20 * 60);

pub const RECORDING_BLOCK: &str = "Stop the recording first, then update.";
pub const NOTES_BLOCK: &str = "Your notes are still being written. Update when they are done.";
pub const ALREADY_NEWEST: &str = "You already have the newest version.";
pub const STOPPED_EARLY: &str = "The update stopped before it finished.";
pub const UPDATING: &str = "Updating... Meeting Notes will restart";
pub const COPY_FALLBACK: &str = "Copied. Paste it into Claude to update.";
pub const COPY_HELP: &str = "Copied. Paste it into Claude.";

/// `state` in `update-status.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusKind {
    Running,
    UpToDate,
    Done,
    Failed,
}

/// The file the updater script and the app share.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub state: StatusKind,
    pub from: String,
    pub to: Option<String>,
    pub error: Option<String>,
    pub log: String,
    pub updated_at: String,
    pub seen: bool,
}

/// What the widget does with a status file read at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartNotice {
    Quiet,
    /// Show the notification, then mark the file seen.
    Updated {
        version: String,
    },
    /// Red "Update failed" until the user gets help or dismisses it.
    Failed {
        message: String,
    },
    /// The script is still going. Keep reading the file.
    InProgress,
}

/// One read while the widget is waiting for the script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    Wait,
    UpToDate,
    Failed {
        message: String,
    },
    /// The relaunched app saw `done` and should announce that version.
    Updated {
        version: String,
    },
}

/// Which updater script and argument style to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptHost {
    Unix,
    Windows,
}

/// How a Unix script is detached. `setsid` fails if it is already a process-group leader,
/// so only the `nohup` path asks for a new process group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnixDetach {
    Setsid,
    Nohup,
}

/// A program and its arguments. The caller spawns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnSpec {
    pub program: String,
    pub args: Vec<String>,
    pub own_process_group: bool,
}

/// Reads one status file body. Unknown states and broken JSON are errors.
pub fn parse_status(text: &str) -> Result<UpdateStatus, String> {
    serde_json::from_str(text).map_err(|err| err.to_string())
}

/// `Ok(None)` when the file is not there yet. A file that is not JSON is an error.
pub fn read_status(path: &Path) -> Result<Option<UpdateStatus>, String> {
    match fs::read_to_string(path) {
        Ok(text) => parse_status(&text).map(Some),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// Writes `path` by creating `path.tmp` and renaming it over the file.
pub fn write_status(path: &Path, status: &UpdateStatus) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(status).map_err(|err| err.to_string())?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

/// Sets `seen` to true. A missing file is left missing.
pub fn mark_seen(path: &Path) -> Result<(), String> {
    let Some(mut status) = read_status(path)? else {
        return Ok(());
    };
    if status.seen {
        return Ok(());
    }
    status.seen = true;
    write_status(path, &status)
}

/// What to tell the user about a status file read once at startup.
///
/// A fresh `running` status means the setup script relaunched the app before it wrote
/// `done`, so the caller keeps polling. `running` older than 20 minutes is a failure.
pub fn notice_on_start(status: &UpdateStatus, now: DateTime<Utc>) -> StartNotice {
    match status.state {
        StatusKind::Done if !status.seen => StartNotice::Updated {
            version: version_of(status),
        },
        StatusKind::Failed if !status.seen => StartNotice::Failed {
            message: failure_line(status),
        },
        StatusKind::Running if is_stale(&status.updated_at, now) => {
            if status.seen {
                StartNotice::Quiet
            } else {
                StartNotice::Failed {
                    message: STOPPED_EARLY.to_string(),
                }
            }
        }
        StatusKind::Running => StartNotice::InProgress,
        StatusKind::Done | StatusKind::Failed | StatusKind::UpToDate => StartNotice::Quiet,
    }
}

/// While this process is waiting: `up_to_date` and `failed` are finished, and `done` means
/// the script is about to quit and relaunch the app.
pub fn poll_outcome(status: &UpdateStatus) -> PollOutcome {
    match status.state {
        StatusKind::UpToDate => PollOutcome::UpToDate,
        StatusKind::Failed => PollOutcome::Failed {
            message: failure_line(status),
        },
        StatusKind::Running | StatusKind::Done => PollOutcome::Wait,
    }
}

/// Same as `poll_outcome`, except a relaunched app announces `done` instead of waiting to be quit.
pub fn watch_step(status: &UpdateStatus, announce_done: bool) -> PollOutcome {
    if announce_done && status.state == StatusKind::Done && !status.seen {
        return PollOutcome::Updated {
            version: version_of(status),
        };
    }
    poll_outcome(status)
}

/// `<home>/Meeting-Minutes`.
pub fn repo_dir(home: &Path) -> PathBuf {
    home.join("Meeting-Minutes")
}

/// `dirs::config_dir()/meeting-notes`, the folder that holds `config.toml`.
pub fn app_config_dir(system_config: &Path) -> PathBuf {
    system_config.join("meeting-notes")
}

/// `<config dir>/update-status.json`.
pub fn status_file(config_dir: &Path) -> PathBuf {
    config_dir.join("update-status.json")
}

/// `scripts/setup/update.sh` or `scripts/setup/update.ps1` inside the clone.
pub fn updater_script(repo: &Path, host: ScriptHost) -> PathBuf {
    let name = match host {
        ScriptHost::Unix => "update.sh",
        ScriptHost::Windows => "update.ps1",
    };
    repo.join("scripts").join("setup").join(name)
}

/// This machine's script style.
pub fn host() -> ScriptHost {
    if cfg!(windows) {
        ScriptHost::Windows
    } else {
        ScriptHost::Unix
    }
}

/// `setsid` when it is on `PATH`, otherwise `nohup`.
pub fn unix_detach(setsid_available: bool) -> UnixDetach {
    if setsid_available {
        UnixDetach::Setsid
    } else {
        UnixDetach::Nohup
    }
}

/// `true` when `name` is a file in one of the `path_env` directories (`sep` is `:` or `;`).
pub fn command_on_path(name: &str, path_env: &str, sep: char) -> bool {
    path_env.split(sep).any(|dir| {
        let dir = dir.trim();
        !dir.is_empty() && Path::new(dir).join(name).is_file()
    })
}

/// Picks `setsid` or `nohup` from a `PATH` string. Windows does not call this.
pub fn detach_for_path(path_env: &str) -> UnixDetach {
    let sep = if cfg!(windows) { ';' } else { ':' };
    unix_detach(command_on_path("setsid", path_env, sep))
}

/// The detached command line. Does not start anything and does not write the status file.
pub fn spawn_spec(
    script: &Path,
    repo: &Path,
    status: &Path,
    host: ScriptHost,
    unix: UnixDetach,
) -> SpawnSpec {
    match host {
        ScriptHost::Windows => SpawnSpec {
            program: "powershell.exe".into(),
            args: vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-WindowStyle".into(),
                "Hidden".into(),
                "-File".into(),
                script.display().to_string(),
                "-Repo".into(),
                repo.display().to_string(),
                "-Status".into(),
                status.display().to_string(),
            ],
            own_process_group: false,
        },
        ScriptHost::Unix => {
            let (program, own_process_group) = match unix {
                UnixDetach::Setsid => ("setsid", false),
                UnixDetach::Nohup => ("nohup", true),
            };
            SpawnSpec {
                program: program.into(),
                args: vec![
                    "bash".into(),
                    script.display().to_string(),
                    "--repo".into(),
                    repo.display().to_string(),
                    "--status".into(),
                    status.display().to_string(),
                ],
                own_process_group,
            }
        }
    }
}

/// Copies the script into `work_dir` and returns the command that runs the copy.
/// The status file is only named in the arguments; this function does not create it.
pub fn prepare_launch(
    script: &Path,
    repo: &Path,
    status: &Path,
    work_dir: &Path,
    host: ScriptHost,
    unix: UnixDetach,
) -> Result<SpawnSpec, String> {
    fs::create_dir_all(work_dir).map_err(|err| format!("{}: {err}", work_dir.display()))?;
    let staged = copy_script(script, work_dir)?;
    Ok(spawn_spec(&staged, repo, status, host, unix))
}

/// `<root>/mn-update-<nonce>`. A new nonce keeps one run from rewriting the previous script.
pub fn fresh_work_dir(root: &Path, nonce: u128) -> PathBuf {
    root.join(format!("mn-update-{nonce}"))
}

/// Recording wins over notes that are still queued. `None` means the update may start.
pub fn refusal(recording: bool, notes_pending: bool) -> Option<&'static str> {
    if recording {
        Some(RECORDING_BLOCK)
    } else if notes_pending {
        Some(NOTES_BLOCK)
    } else {
        None
    }
}

/// Notes are in the way when one is being written or others are waiting behind it.
pub fn notes_pending(processing: bool, waiting: usize) -> bool {
    processing || waiting > 0
}

/// "Meeting Notes is updated to v0.1.8."
pub fn updated_message(version: &str) -> String {
    format!("Meeting Notes is updated to {version}.")
}

/// Tray item: "Update to v0.1.8".
pub fn tray_label(version: &str) -> String {
    format!("Update to {version}")
}

/// Windows process flags from the shared contract:
/// `CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`.
///
/// Compiled for Windows, and for tests on every OS so the flag set stays checked.
#[cfg(any(windows, test))]
pub fn windows_creation_flags() -> u32 {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    CREATE_NO_WINDOW | DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
}

fn version_of(status: &UpdateStatus) -> String {
    status
        .to
        .clone()
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "the newest version".to_string())
}

fn failure_line(status: &UpdateStatus) -> String {
    status
        .error
        .clone()
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "Update failed".to_string())
}

/// `updated_at` is stale when it is more than 20 minutes before `now`, or cannot be read.
fn is_stale(updated_at: &str, now: DateTime<Utc>) -> bool {
    let Ok(parsed) = DateTime::parse_from_rfc3339(updated_at) else {
        return true;
    };
    let Ok(limit) = chrono::Duration::from_std(STALE_RUNNING) else {
        return true;
    };
    now.signed_duration_since(parsed.with_timezone(&Utc)) > limit
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
        }
    }
    let tmp = tmp_sibling(path);
    write_tmp(&tmp, bytes).map_err(|err| format!("{}: {err}", tmp.display()))?;
    replace_file(&tmp, path).map_err(|err| format!("{}: {err}", path.display()))
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

fn write_tmp(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Rename `tmp` over `path`. Windows cannot replace an existing file with `rename`, so the
/// old file is removed first.
fn replace_file(tmp: &Path, path: &Path) -> io::Result<()> {
    #[cfg(windows)]
    {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    fs::rename(tmp, path)
}

fn copy_script(script: &Path, dest_dir: &Path) -> Result<PathBuf, String> {
    let name = script
        .file_name()
        .ok_or_else(|| format!("{} has no file name", script.display()))?;
    let dest = dest_dir.join(name);
    fs::copy(script, &dest).map_err(|err| format!("{}: {err}", script.display()))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const DONE: &str = r#"{
  "state": "done",
  "from": "v0.1.7",
  "to": "v0.1.8",
  "error": null,
  "log": "/home/jane/.config/meeting-notes/update.log",
  "updated_at": "2026-10-24T10:15:00Z",
  "seen": false
}"#;

    fn sample() -> UpdateStatus {
        UpdateStatus {
            state: StatusKind::Running,
            from: "v0.1.7".into(),
            to: None,
            error: None,
            log: "/home/jane/.config/meeting-notes/update.log".into(),
            updated_at: "2026-10-24T10:00:00Z".into(),
            seen: false,
        }
    }

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn parts(path: &Path) -> Vec<String> {
        path.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_contract_status_file_parses_field_by_field() {
        let status = parse_status(DONE).unwrap();
        assert_eq!(status.state, StatusKind::Done);
        assert_eq!(status.from, "v0.1.7");
        assert_eq!(status.to.as_deref(), Some("v0.1.8"));
        assert_eq!(status.error, None);
        assert_eq!(status.log, "/home/jane/.config/meeting-notes/update.log");
        assert_eq!(status.updated_at, "2026-10-24T10:15:00Z");
        assert!(!status.seen);
    }

    #[test]
    fn running_and_up_to_date_use_the_contract_names() {
        let running = r#"{"state":"running","from":"unknown","to":null,"error":null,"log":"/tmp/update.log","updated_at":"2026-10-24T10:00:00Z","seen":false}"#;
        assert_eq!(parse_status(running).unwrap().state, StatusKind::Running);
        let current = r#"{"state":"up_to_date","from":"v0.1.8","to":"v0.1.8","error":null,"log":"/tmp/update.log","updated_at":"2026-10-24T10:00:00Z","seen":false}"#;
        assert_eq!(parse_status(current).unwrap().state, StatusKind::UpToDate);
    }

    #[test]
    fn a_failed_status_keeps_its_error_line() {
        let text = r#"{"state":"failed","from":"unknown","to":null,"error":"This version is still being prepared","log":"/tmp/update.log","updated_at":"2026-10-24T10:00:00Z","seen":false}"#;
        let status = parse_status(text).unwrap();
        assert_eq!(status.state, StatusKind::Failed);
        assert_eq!(status.from, "unknown");
        assert_eq!(
            status.error.as_deref(),
            Some("This version is still being prepared")
        );
    }

    #[test]
    fn garbage_and_an_unknown_state_are_errors() {
        assert!(parse_status("").is_err());
        assert!(parse_status("not json").is_err());
        assert!(parse_status(r#"{"state":"nope"}"#).is_err());
    }

    #[test]
    fn a_missing_status_file_is_none_and_a_broken_one_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-status.json");
        assert_eq!(read_status(&path).unwrap(), None);
        fs::write(&path, "nope").unwrap();
        assert!(read_status(&path).is_err());
    }

    #[test]
    fn the_status_file_is_replaced_atomically_and_the_temp_file_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("update-status.json");
        let mut status = sample();
        write_status(&path, &status).unwrap();
        status.state = StatusKind::Done;
        status.to = Some("v0.1.8".into());
        status.seen = false;
        write_status(&path, &status).unwrap();
        let got = read_status(&path).unwrap().unwrap();
        assert_eq!(got, status);
        assert!(!path.with_extension("json.tmp").exists());
        let tmp = format!("{}.tmp", path.display());
        assert!(!Path::new(&tmp).exists());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"seen\":false") || raw.contains("\"seen\": false"));
        assert!(raw.contains("\"to\":null") || raw.contains("\"to\": null") || status.to.is_some());
        assert!(raw.contains("\"state\":\"done\"") || raw.contains("\"state\": \"done\""));
    }

    #[test]
    fn null_to_is_written_as_null() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("update-status.json");
        write_status(&path, &sample()).unwrap();
        let raw = fs::read_to_string(&path).unwrap();
        assert!(raw.contains("\"to\":null") || raw.contains("\"to\": null"));
        assert!(raw.contains("\"error\":null") || raw.contains("\"error\": null"));
    }

    #[test]
    fn mark_seen_flips_only_seen_and_leaves_a_missing_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.json");
        assert!(mark_seen(&missing).is_ok());
        assert!(!missing.exists());

        let path = dir.path().join("update-status.json");
        write_status(&path, &sample()).unwrap();
        mark_seen(&path).unwrap();
        let once = read_status(&path).unwrap().unwrap();
        assert!(once.seen);
        assert_eq!(once.from, "v0.1.7");
        assert_eq!(once.state, StatusKind::Running);
        assert_eq!(once.updated_at, "2026-10-24T10:00:00Z");
        mark_seen(&path).unwrap();
        let twice = read_status(&path).unwrap().unwrap();
        assert_eq!(twice, once);
    }

    #[test]
    fn an_unseen_done_status_announces_the_version() {
        let mut status = sample();
        status.state = StatusKind::Done;
        status.to = Some("v0.1.8".into());
        assert_eq!(
            notice_on_start(&status, at("2026-10-24T12:00:00Z")),
            StartNotice::Updated {
                version: "v0.1.8".into()
            }
        );
        status.seen = true;
        assert_eq!(
            notice_on_start(&status, at("2026-10-24T12:00:00Z")),
            StartNotice::Quiet
        );
    }

    #[test]
    fn an_unseen_failure_is_shown_until_it_has_been_seen() {
        let mut status = sample();
        status.state = StatusKind::Failed;
        status.error = Some("Could not download the release.".into());
        assert_eq!(
            notice_on_start(&status, at("2026-10-24T12:00:00Z")),
            StartNotice::Failed {
                message: "Could not download the release.".into()
            }
        );
        status.seen = true;
        assert_eq!(
            notice_on_start(&status, at("2026-10-24T12:00:00Z")),
            StartNotice::Quiet
        );
    }

    #[test]
    fn a_running_status_is_in_progress_until_it_is_older_than_twenty_minutes() {
        let status = sample();
        let start = at("2026-10-24T10:00:00Z");
        assert_eq!(
            notice_on_start(&status, start + chrono::Duration::minutes(20)),
            StartNotice::InProgress
        );
        assert_eq!(
            notice_on_start(
                &status,
                start + chrono::Duration::minutes(20) + chrono::Duration::seconds(1)
            ),
            StartNotice::Failed {
                message: STOPPED_EARLY.into()
            }
        );
    }

    #[test]
    fn a_stale_running_status_that_was_already_seen_stays_quiet() {
        let mut status = sample();
        status.seen = true;
        let start = at("2026-10-24T10:00:00Z");
        assert_eq!(
            notice_on_start(&status, start + chrono::Duration::minutes(30)),
            StartNotice::Quiet
        );
    }

    #[test]
    fn an_unreadable_running_timestamp_is_treated_as_stopped() {
        let mut status = sample();
        status.updated_at = "yesterday".into();
        assert_eq!(
            notice_on_start(&status, at("2026-10-24T12:00:00Z")),
            StartNotice::Failed {
                message: STOPPED_EARLY.into()
            }
        );
    }

    #[test]
    fn up_to_date_on_startup_does_not_announce() {
        let mut status = sample();
        status.state = StatusKind::UpToDate;
        status.to = Some("v0.1.7".into());
        assert_eq!(
            notice_on_start(&status, at("2026-10-24T12:00:00Z")),
            StartNotice::Quiet
        );
    }

    #[test]
    fn polling_waits_on_running_and_done_and_stops_on_the_other_two() {
        let mut status = sample();
        assert_eq!(poll_outcome(&status), PollOutcome::Wait);
        status.state = StatusKind::Done;
        status.to = Some("v0.1.8".into());
        assert_eq!(poll_outcome(&status), PollOutcome::Wait);
        status.state = StatusKind::UpToDate;
        assert_eq!(poll_outcome(&status), PollOutcome::UpToDate);
        status.state = StatusKind::Failed;
        status.error = Some("doctor failed".into());
        assert_eq!(
            poll_outcome(&status),
            PollOutcome::Failed {
                message: "doctor failed".into()
            }
        );
        status.error = None;
        assert_eq!(
            poll_outcome(&status),
            PollOutcome::Failed {
                message: "Update failed".into()
            }
        );
    }

    #[test]
    fn paths_point_at_the_clone_and_the_config_dir() {
        assert_eq!(
            parts(&repo_dir(Path::new("jane"))),
            ["jane", "Meeting-Minutes"]
        );
        let config = app_config_dir(&Path::new("jane").join(".config"));
        assert_eq!(
            parts(&status_file(&config)),
            ["jane", ".config", "meeting-notes", "update-status.json"]
        );
        let repo = repo_dir(Path::new("jane"));
        assert_eq!(
            parts(&updater_script(&repo, ScriptHost::Unix)),
            ["jane", "Meeting-Minutes", "scripts", "setup", "update.sh"]
        );
        assert_eq!(
            parts(&updater_script(&repo, ScriptHost::Windows)),
            ["jane", "Meeting-Minutes", "scripts", "setup", "update.ps1"]
        );
    }

    #[test]
    fn the_work_dir_is_a_fresh_temp_folder() {
        assert_eq!(
            fresh_work_dir(Path::new("/tmp"), 42),
            PathBuf::from("/tmp/mn-update-42")
        );
    }

    #[test]
    fn setsid_is_preferred_and_only_when_it_is_on_path() {
        assert_eq!(unix_detach(true), UnixDetach::Setsid);
        assert_eq!(unix_detach(false), UnixDetach::Nohup);
        let dir = tempfile::tempdir().unwrap();
        assert!(!command_on_path(
            "setsid",
            dir.path().to_str().unwrap(),
            ':'
        ));
        fs::write(dir.path().join("setsid"), b"#!/bin/sh\n").unwrap();
        assert!(command_on_path("setsid", dir.path().to_str().unwrap(), ':'));
        assert!(!command_on_path("setsid", "", ':'));
    }

    #[test]
    fn bash_is_started_detached_with_repo_and_status() {
        let script = Path::new("/tmp/mn-update-1/update.sh");
        let repo = Path::new("/home/jane/Meeting-Minutes");
        let status = Path::new("/home/jane/.config/meeting-notes/update-status.json");
        let setsid = spawn_spec(script, repo, status, ScriptHost::Unix, UnixDetach::Setsid);
        assert_eq!(setsid.program, "setsid");
        assert!(!setsid.own_process_group);
        assert_eq!(
            setsid.args,
            [
                "bash",
                "/tmp/mn-update-1/update.sh",
                "--repo",
                "/home/jane/Meeting-Minutes",
                "--status",
                "/home/jane/.config/meeting-notes/update-status.json",
            ]
        );
        let nohup = spawn_spec(script, repo, status, ScriptHost::Unix, UnixDetach::Nohup);
        assert_eq!(nohup.program, "nohup");
        assert!(nohup.own_process_group);
        assert_eq!(nohup.args, setsid.args);
    }

    #[test]
    fn powershell_is_started_hidden_with_repo_and_status() {
        let spec = spawn_spec(
            Path::new(r"C:\Temp\update.ps1"),
            Path::new(r"C:\Users\jane\Meeting-Minutes"),
            Path::new(r"C:\Users\jane\AppData\Roaming\meeting-notes\update-status.json"),
            ScriptHost::Windows,
            UnixDetach::Setsid,
        );
        assert_eq!(spec.program, "powershell.exe");
        assert!(!spec.own_process_group);
        assert_eq!(
            spec.args,
            [
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-WindowStyle",
                "Hidden",
                "-File",
                r"C:\Temp\update.ps1",
                "-Repo",
                r"C:\Users\jane\Meeting-Minutes",
                "-Status",
                r"C:\Users\jane\AppData\Roaming\meeting-notes\update-status.json",
            ]
        );
        assert_eq!(
            windows_creation_flags(),
            0x0800_0000 | 0x0000_0008 | 0x0000_0200
        );
    }

    #[test]
    fn prepare_launch_copies_the_script_and_does_not_touch_the_status_file() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("Meeting-Minutes");
        let script = repo.join("scripts").join("setup").join("update.sh");
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, b"#!/bin/bash\necho hi\n").unwrap();
        let status = dir.path().join("update-status.json");
        let work = dir.path().join("work");
        let spec = prepare_launch(
            &script,
            &repo,
            &status,
            &work,
            ScriptHost::Unix,
            UnixDetach::Setsid,
        )
        .unwrap();
        assert!(!status.exists());
        let staged = work.join("update.sh");
        assert_eq!(fs::read(&staged).unwrap(), b"#!/bin/bash\necho hi\n");
        assert_eq!(spec.args[1], staged.display().to_string());
        assert_eq!(spec.program, "setsid");
        assert_eq!(spec.args[3], repo.display().to_string());
        assert_eq!(spec.args[5], status.display().to_string());
    }

    #[test]
    fn a_relaunched_app_announces_done_and_the_starter_keeps_waiting() {
        let mut status = sample();
        status.state = StatusKind::Done;
        status.to = Some("v0.1.8".into());
        assert_eq!(watch_step(&status, false), PollOutcome::Wait);
        assert_eq!(
            watch_step(&status, true),
            PollOutcome::Updated {
                version: "v0.1.8".into()
            }
        );
        status.seen = true;
        assert_eq!(watch_step(&status, true), PollOutcome::Wait);
    }

    #[test]
    fn a_recording_blocks_before_queued_notes_do() {
        assert_eq!(refusal(true, true), Some(RECORDING_BLOCK));
        assert_eq!(refusal(true, false), Some(RECORDING_BLOCK));
        assert_eq!(refusal(false, true), Some(NOTES_BLOCK));
        assert_eq!(refusal(false, false), None);
        assert!(notes_pending(true, 0));
        assert!(notes_pending(false, 2));
        assert!(!notes_pending(false, 0));
    }

    #[test]
    fn the_sentences_the_widget_shows() {
        assert_eq!(
            updated_message("v0.1.8"),
            "Meeting Notes is updated to v0.1.8."
        );
        assert_eq!(tray_label("v0.1.8"), "Update to v0.1.8");
        assert_eq!(ALREADY_NEWEST, "You already have the newest version.");
        assert_eq!(COPY_FALLBACK, "Copied. Paste it into Claude to update.");
        assert_eq!(COPY_HELP, "Copied. Paste it into Claude.");
        assert_eq!(UPDATING, "Updating... Meeting Notes will restart");
    }
}
