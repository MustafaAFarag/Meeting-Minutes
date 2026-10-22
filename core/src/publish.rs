use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Local, SecondsFormat};
use serde_json::{json, Value};

use crate::config::Config;
use crate::exec::{claude_command, reason, run};
use crate::status::Status;
use crate::summarize::{MEETING_FILE, PUBLISH_PENDING, TRANSCRIPT_FILE};

const INDEX_HTML: &str = include_str!("../../artifact/index.html");

const ARTIFACT_URL_PREFIX: &str = "https://claude.ai/artifact/";
const ARTIFACT_URL_FILE: &str = "artifact_url.txt";
/// Staging dirs live in `<notes_dir>/.staging`; sessions never start with a dot.
const STAGING_DIR: &str = ".staging";

/// Publishes every pending session (meeting.json plus a PUBLISH_PENDING marker) to the
/// "My Meetings" artifact. The markers are removed only after a successful publish.
pub fn publish(cfg: &Config, on_status: impl Fn(Status)) -> Result<()> {
    let config_dir = dirs::config_dir().context("could not find the user config directory")?;
    publish_with_config_dir(cfg, &config_dir, on_status)
}

/// Same as `publish`, with the user config directory given (it holds `meeting-notes/artifact_url.txt`).
pub fn publish_with_config_dir(
    cfg: &Config,
    config_dir: &Path,
    on_status: impl Fn(Status),
) -> Result<()> {
    let sessions = sessions_with_meeting(&cfg.notes_dir)?;
    let pending: Vec<&Session> = sessions.iter().filter(|s| s.pending).collect();
    if pending.is_empty() {
        return Ok(());
    }
    on_status(Status::Publishing {
        meetings: pending.len(),
    });

    let staging = Staging::create(&cfg.notes_dir)?;
    let files = stage_files(&staging.path, &sessions)?;
    let url_file = config_dir.join("meeting-notes").join(ARTIFACT_URL_FILE);
    let known_url = known_url(cfg, &url_file);
    let files_json = serde_json::to_string(&files)?;
    let prompt = match &known_url {
        Some(url) => update_prompt(url, &files_json),
        None => create_prompt(&files_json),
    };

    let mut cmd = claude_command(cfg);
    cmd.arg("-p")
        .arg(&prompt)
        .args(["--allowedTools", "Artifact"])
        .env("CLAUDE_CODE_ARTIFACT", "1")
        .current_dir(&staging.path);
    let out = run(&mut cmd, None)
        .with_context(|| format!("could not run claude_cli {}", cfg.claude_cli.display()))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !stdout.trim().starts_with("OK") {
        bail!("publishing failed ({}): {}", out.status, reason(&out));
    }

    let url = match known_url {
        Some(url) => url,
        None => {
            let url = parse_artifact_url(&stdout).ok_or_else(|| {
                anyhow!(
                    "the artifact was published but its URL was not in the reply (set artifact_url in the config or write it to {}): {}",
                    url_file.display(),
                    crate::exec::snippet(&stdout)
                )
            })?;
            save_url(&url_file, &url)?;
            url
        }
    };

    for session in &pending {
        let marker = session.dir.join(PUBLISH_PENDING);
        fs::remove_file(&marker).with_context(|| format!("removing {}", marker.display()))?;
    }
    on_status(Status::Published { url });
    Ok(())
}

/// The first https://claude.ai/artifact/... URL in `text`.
pub fn parse_artifact_url(text: &str) -> Option<String> {
    let start = text.find(ARTIFACT_URL_PREFIX)?;
    let url: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, ')' | ']' | '>' | '"' | '\'' | ','))
        .collect();
    let url = url.trim_end_matches(['.', ';', ':']);
    (url.len() > ARTIFACT_URL_PREFIX.len()).then(|| url.to_string())
}

fn update_prompt(url: &str, files_json: &str) -> String {
    format!(
        "Update the existing artifact {url}: first read it with action read, then list its files with action list, scope files and url set to it, so the publish knows what is already there. Then publish with url set to it, file_path ./index.html (unchanged, do not edit it, omit capabilities so the stored ones are kept), and files {files_json}. If the publish is still refused because some files were not read, read each path it names with action read and path, then publish again exactly the same way. Reply with only OK plus the list of published files, or the exact error."
    )
}

fn create_prompt(files_json: &str) -> String {
    format!(
        "Publish ./index.html as a new artifact using the Artifact tool, unchanged, with capabilities {{\"db\":{{}},\"user\":{{}},\"downloads\":true}}, icon \"calendar\", title \"My Meetings\", and files {files_json}. Reply with only OK and the artifact URL on the next line, or the exact error."
    )
}

/// `artifact_url` from the config, else the first line of the saved URL file.
fn known_url(cfg: &Config, url_file: &Path) -> Option<String> {
    let from_config = cfg.artifact_url.as_deref().map(str::trim);
    if let Some(url) = from_config.filter(|u| !u.is_empty()) {
        return Some(url.to_string());
    }
    let text = fs::read_to_string(url_file).ok()?;
    let first = text.lines().next()?.trim();
    (!first.is_empty()).then(|| first.to_string())
}

fn save_url(url_file: &Path, url: &str) -> Result<()> {
    if let Some(parent) = url_file.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(url_file, format!("{url}\n"))
        .with_context(|| format!("writing {}", url_file.display()))
}

struct Session {
    dir: PathBuf,
    id: String,
    pending: bool,
    meeting: Value,
}

/// Every session dir in the notes dir that has a readable meeting.json.
/// A pending session with a broken meeting.json is an error; an old one is skipped.
fn sessions_with_meeting(notes_dir: &Path) -> Result<Vec<Session>> {
    let entries = match fs::read_dir(notes_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(e).with_context(|| format!("reading notes dir {}", notes_dir.display()))
        }
    };
    let mut sessions = Vec::new();
    for entry in entries {
        let dir = entry
            .with_context(|| format!("reading notes dir {}", notes_dir.display()))?
            .path();
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let meeting_path = dir.join(MEETING_FILE);
        if id.starts_with('.') || !dir.is_dir() || !meeting_path.exists() {
            continue;
        }
        let pending = dir.join(PUBLISH_PENDING).exists();
        let meeting = fs::read_to_string(&meeting_path)
            .with_context(|| format!("reading {}", meeting_path.display()))
            .and_then(|text| {
                serde_json::from_str::<Value>(&text)
                    .with_context(|| format!("parsing {}", meeting_path.display()))
            });
        match meeting {
            Ok(meeting) => sessions.push(Session {
                dir,
                id,
                pending,
                meeting,
            }),
            Err(e) if pending => return Err(e),
            Err(_) => continue,
        }
    }
    Ok(sessions)
}

/// A unique temp dir under `<notes_dir>/.staging`, removed on drop.
struct Staging {
    path: PathBuf,
    parent: PathBuf,
}

impl Staging {
    fn create(notes_dir: &Path) -> Result<Self> {
        let parent = notes_dir.join(STAGING_DIR);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = parent.join(format!("publish-{}-{nanos}", process::id()));
        fs::create_dir_all(&path)
            .with_context(|| format!("creating staging dir {}", path.display()))?;
        Ok(Staging { path, parent })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
        // only succeeds when no other publish is staging
        let _ = fs::remove_dir(&self.parent);
    }
}

fn write_staged(root: &Path, relative: &str, content: impl AsRef<[u8]>) -> Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    fs::write(&path, content).with_context(|| format!("writing {}", path.display()))
}

/// Fills the staging dir and returns the files map (path -> same relative path) for the prompt.
fn stage_files(root: &Path, sessions: &[Session]) -> Result<BTreeMap<String, String>> {
    write_staged(root, "index.html", INDEX_HTML)?;

    let mut files = BTreeMap::new();
    let mut add = |relative: String| {
        files.insert(relative.clone(), relative);
    };
    write_staged(root, "data/index.json", index_json(sessions)?)?;
    add("data/index.json".to_string());

    for session in sessions.iter().filter(|s| s.pending) {
        let meeting_rel = format!("data/meetings/{}.json", session.id);
        write_staged(
            root,
            &meeting_rel,
            serde_json::to_string_pretty(&session.meeting)?,
        )?;
        add(meeting_rel);

        let transcript_path = session.dir.join(TRANSCRIPT_FILE);
        let transcript = fs::read_to_string(&transcript_path)
            .with_context(|| format!("reading {}", transcript_path.display()))?;
        let transcript_rel = format!("data/transcripts/{}.md", session.id);
        write_staged(root, &transcript_rel, transcript)?;
        add(transcript_rel);
    }
    Ok(files)
}

/// All meetings, newest first.
fn index_json(sessions: &[Session]) -> Result<String> {
    let mut entries: Vec<(Option<DateTime<chrono::FixedOffset>>, &str, Value)> = sessions
        .iter()
        .map(|s| {
            let started = s.meeting["started_at"]
                .as_str()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok());
            let mut entry = json!({"id": s.id});
            for key in ["title", "started_at", "generated_at"] {
                if let Some(value) = s.meeting.get(key).filter(|v| !v.is_null()) {
                    entry[key] = value.clone();
                }
            }
            (started, s.id.as_str(), entry)
        })
        .collect();
    entries.sort_by(|a, b| (b.0, b.1).cmp(&(a.0, a.1)));
    let meetings: Vec<Value> = entries.into_iter().map(|(_, _, entry)| entry).collect();
    let index = json!({
        "updated_at": Local::now().to_rfc3339_opts(SecondsFormat::Secs, false),
        "meetings": meetings,
    });
    Ok(serde_json::to_string_pretty(&index)?)
}
