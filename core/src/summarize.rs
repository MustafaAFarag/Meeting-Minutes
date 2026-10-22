use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Local, SecondsFormat};
use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::config::Config;
use crate::exec::{claude_command, reason, resolves, run, snippet};
use crate::session::SessionInfo;
use crate::status::Status;

const TEMPLATE: &str = include_str!("../../prompts/meeting-card.md");
const SCHEMA: &str = include_str!("../../prompts/card.schema.json");
const M365_STEPS: &str = include_str!("../../docs/m365-step.md");

const CLAUDE_TOOLS: &str = "Read,mcp__claude_ai_Microsoft_365__outlook_calendar_search,mcp__claude_ai_Microsoft_365__chat_message_search,mcp__claude_ai_Microsoft_365__sharepoint_search";

pub const CARD_FILE: &str = "card.json";
pub const MEETING_FILE: &str = "meeting.json";
pub const TRANSCRIPT_FILE: &str = "transcript.md";
/// Written by `summarize`, removed by `publish` once the meeting is on the artifact page.
pub const PUBLISH_PENDING: &str = "PUBLISH_PENDING";

/// Temp files for codex, in the session dir, deleted after the call.
const CODEX_SCHEMA_FILE: &str = ".card.schema.json";
const CODEX_OUT_FILE: &str = ".codex-card.json";

/// Longest `reason` on the fallback status line.
const STATUS_REASON_CHARS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    Claude,
    Codex,
}

impl Engine {
    fn section(self) -> &'static str {
        match self {
            Engine::Claude => "claude",
            Engine::Codex => "codex",
        }
    }
}

/// The text of one `## <engine>` section of docs/m365-step.md.
pub fn m365_step(engine: Engine) -> Result<String> {
    let heading = format!("## {}", engine.section());
    let mut lines = M365_STEPS.lines();
    lines
        .by_ref()
        .find(|line| line.trim_end() == heading)
        .ok_or_else(|| anyhow!("docs/m365-step.md has no `{heading}` section"))?;
    let body: Vec<&str> = lines.take_while(|line| !line.starts_with('#')).collect();
    Ok(body.join("\n").trim().to_string())
}

/// Replaces `{{NAME}}` with its value in one pass, so a value is never searched for placeholders.
/// Unknown placeholders stay as they are.
pub fn fill_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            rest = &rest[open..];
            break;
        };
        match vars.iter().find(|(name, _)| *name == &after[..close]) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[open..open + 2 + close + 2]),
        }
        rest = &after[close + 2..];
    }
    out.push_str(rest);
    out
}

/// `session_dir` should be absolute: the prompt tells the model where to read from.
pub fn build_prompt(
    cfg: &Config,
    session_dir: &Path,
    info: &SessionInfo,
    engine: Engine,
) -> Result<String> {
    let started = DateTime::parse_from_rfc3339(&info.started_at)
        .with_context(|| format!("started_at `{}` is not RFC3339", info.started_at))?;
    let today = started.format("%Y-%m-%d (%A)").to_string();
    let session = session_dir.display().to_string();
    let step = m365_step(engine)?;
    Ok(fill_template(
        TEMPLATE,
        &[
            ("SESSION_DIR", &session),
            ("STARTED_AT", &info.started_at),
            ("ENDED_AT", &info.ended_at),
            ("TODAY", &today),
            ("MY_NAME", cfg.my_name.as_deref().unwrap_or("unknown")),
            ("M365_STEP", &step),
        ],
    ))
}

/// Writes card.json, meeting.json and the PUBLISH_PENDING marker into the session dir.
/// Claude first, Codex if Claude fails. Nothing is written when both fail.
pub fn summarize(cfg: &Config, session_dir: &Path, on_status: impl Fn(Status)) -> Result<PathBuf> {
    let session_dir = fs::canonicalize(session_dir)
        .with_context(|| format!("session dir {}", session_dir.display()))?;
    let info = SessionInfo::load(&session_dir)?
        .ok_or_else(|| anyhow!("no session.json in {}", session_dir.display()))?;
    let transcript = session_dir.join(TRANSCRIPT_FILE);
    if !transcript.exists() {
        bail!(
            "no {TRANSCRIPT_FILE} in {}; run `mn transcribe` first",
            session_dir.display()
        );
    }

    on_status(Status::Summarizing {
        engine: Engine::Claude,
        reason: None,
    });
    let claude_prompt = build_prompt(cfg, &session_dir, &info, Engine::Claude)?;
    let (engine, card) = match run_claude(cfg, &session_dir, &claude_prompt) {
        Ok(card) => (Engine::Claude, card),
        Err(claude_err) => {
            let claude_reason = format!("{claude_err:#}");
            if !resolves(&cfg.codex_cli) {
                bail!(
                    "claude failed ({claude_reason}); codex fallback not available: {} not found",
                    cfg.codex_cli.display()
                );
            }
            on_status(Status::Summarizing {
                engine: Engine::Codex,
                reason: Some(short(&claude_reason)),
            });
            let codex_prompt = build_prompt(cfg, &session_dir, &info, Engine::Codex)?;
            match run_codex(cfg, &session_dir, &codex_prompt) {
                Ok(card) => (Engine::Codex, card),
                Err(codex_err) => {
                    bail!("claude failed ({claude_reason}); codex failed ({codex_err:#})")
                }
            }
        }
    };

    let card_path = session_dir.join(CARD_FILE);
    write_json(&card_path, &card)?;
    let meeting = meeting_json(&card, &session_dir, &info, engine)?;
    write_json(&session_dir.join(MEETING_FILE), &meeting)?;
    let marker = session_dir.join(PUBLISH_PENDING);
    fs::write(&marker, "").with_context(|| format!("writing {}", marker.display()))?;

    on_status(Status::Summarized {
        engine,
        card: card_path.clone(),
    });
    Ok(card_path)
}

fn short(reason: &str) -> String {
    let first_line = reason.lines().next().unwrap_or_default();
    first_line.chars().take(STATUS_REASON_CHARS).collect()
}

fn run_claude(cfg: &Config, session_dir: &Path, prompt: &str) -> Result<Value> {
    let schema: Value = serde_json::from_str(SCHEMA).context("embedded card schema")?;
    let mut cmd = claude_command(cfg);
    cmd.args(["-p", "--output-format", "json", "--json-schema"])
        .arg(schema.to_string())
        .args(["--allowedTools", CLAUDE_TOOLS])
        .current_dir(session_dir);
    let out = run(&mut cmd, Some(prompt))
        .with_context(|| format!("could not run claude_cli {}", cfg.claude_cli.display()))?;
    if !out.status.success() {
        bail!("claude exited with {}: {}", out.status, reason(&out));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let reply: Value = serde_json::from_str(&stdout)
        .map_err(|e| anyhow!("claude output is not JSON ({e}): {}", snippet(&stdout)))?;
    if reply["is_error"].as_bool().unwrap_or(false) {
        bail!("claude reported an error: {}", snippet(&stdout));
    }
    match reply.get("structured_output") {
        Some(card) if card.is_object() => Ok(card.clone()),
        _ => bail!("claude returned no structured_output: {}", snippet(&stdout)),
    }
}

fn run_codex(cfg: &Config, session_dir: &Path, prompt: &str) -> Result<Value> {
    let schema_path = session_dir.join(CODEX_SCHEMA_FILE);
    let out_path = session_dir.join(CODEX_OUT_FILE);
    let result = run_codex_with_files(cfg, session_dir, prompt, &schema_path, &out_path);
    let _ = fs::remove_file(&schema_path);
    let _ = fs::remove_file(&out_path);
    result
}

fn run_codex_with_files(
    cfg: &Config,
    session_dir: &Path,
    prompt: &str,
    schema_path: &Path,
    out_path: &Path,
) -> Result<Value> {
    fs::write(schema_path, SCHEMA).with_context(|| format!("writing {}", schema_path.display()))?;
    let mut cmd = Command::new(&cfg.codex_cli);
    cmd.args(["exec", "--skip-git-repo-check", "-C"])
        .arg(session_dir)
        .arg("--output-schema")
        .arg(schema_path)
        .arg("-o")
        .arg(out_path)
        .arg("-")
        .current_dir(session_dir);
    let out = run(&mut cmd, Some(prompt))
        .with_context(|| format!("could not run codex_cli {}", cfg.codex_cli.display()))?;
    if !out.status.success() {
        bail!("codex exited with {}: {}", out.status, reason(&out));
    }
    let text = fs::read_to_string(out_path)
        .with_context(|| format!("codex wrote no {}", out_path.display()))?;
    let card: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("codex output is not JSON ({e}): {}", snippet(&text)))?;
    if !card.is_object() {
        bail!("codex output is not a JSON object: {}", snippet(&text));
    }
    Ok(card)
}

/// The card plus what the page needs to list it.
fn meeting_json(
    card: &Value,
    session_dir: &Path,
    info: &SessionInfo,
    engine: Engine,
) -> Result<Value> {
    let start = DateTime::parse_from_rfc3339(&info.started_at)
        .with_context(|| format!("started_at `{}` is not RFC3339", info.started_at))?;
    let end = DateTime::parse_from_rfc3339(&info.ended_at)
        .with_context(|| format!("ended_at `{}` is not RFC3339", info.ended_at))?;
    let minutes = ((end - start).num_seconds() as f64 / 60.0).round() as i64;
    let id = session_dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut meeting: Map<String, Value> = card.as_object().cloned().unwrap_or_default();
    meeting.insert("id".into(), json!(id));
    meeting.insert("started_at".into(), json!(info.started_at));
    meeting.insert("ended_at".into(), json!(info.ended_at));
    meeting.insert("duration_min".into(), json!(minutes.max(0)));
    meeting.insert("generated_by".into(), json!(engine));
    meeting.insert(
        "generated_at".into(),
        json!(Local::now().to_rfc3339_opts(SecondsFormat::Secs, false)),
    );
    Ok(Value::Object(meeting))
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}
