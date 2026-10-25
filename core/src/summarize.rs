use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use chrono::{DateTime, Duration, Local, SecondsFormat};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::config::Config;
use crate::exec::{claude_command, command, reason, resolves, run};
use crate::failure::{classify, ErrorKind, Failure, Stage};
use crate::logging::{self, session_name};
use crate::session::{SessionInfo, TrackSeconds};
use crate::status::Status;

const TEMPLATE: &str = include_str!("../../prompts/meeting-card.md");
const SCHEMA: &str = include_str!("../../prompts/card.schema.json");
const M365_STEPS: &str = include_str!("../../docs/m365-step.md");

/// The card run gets no built-in tools (`--tools ""`): the transcript is in the prompt, so it needs no file access.
/// Only these read-only Microsoft 365 searches are allowed.
const CLAUDE_TOOLS: &str = "mcp__claude_ai_Microsoft_365__outlook_calendar_search,mcp__claude_ai_Microsoft_365__chat_message_search,mcp__claude_ai_Microsoft_365__sharepoint_search";

pub const CARD_FILE: &str = "card.json";
pub const MEETING_FILE: &str = "meeting.json";
pub const TRANSCRIPT_FILE: &str = "transcript.md";
const FULL_TRANSCRIPT_FILE: &str = "transcript.full.md";
/// Written by `summarize`, removed by `publish` once the meeting is on the artifact page.
pub const PUBLISH_PENDING: &str = "PUBLISH_PENDING";

/// Temp files for codex, in the session dir, deleted after the call.
const CODEX_SCHEMA_FILE: &str = ".card.schema.json";
const CODEX_OUT_FILE: &str = ".codex-card.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// `session_dir` should be absolute: the prompt names it. The transcript goes inside the prompt as data.
pub fn build_prompt(
    cfg: &Config,
    session_dir: &Path,
    info: &SessionInfo,
    transcript: &str,
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
            ("TRANSCRIPT", transcript.trim_end()),
        ],
    ))
}

/// Writes card.json, meeting.json and the PUBLISH_PENDING marker into the session dir.
/// Claude first, Codex if Claude fails. Nothing is written when both fail.
pub fn summarize(cfg: &Config, session_dir: &Path, on_status: impl Fn(Status)) -> Result<PathBuf> {
    let started = Instant::now();
    let name = session_name(session_dir);
    logging::info(
        "summarize",
        &format!(
            "start session={name} engine=claude model={}",
            cfg.card_model
        ),
    );
    let result = summarize_inner(cfg, session_dir, on_status);
    match result {
        Ok(path) => {
            logging::info(
                "summarize",
                &format!(
                    "end session={name} success=true duration_ms={}",
                    started.elapsed().as_millis()
                ),
            );
            Ok(path)
        }
        Err(err) => {
            let kind = classify(&err, Stage::Summarize).0;
            logging::error(
                "summarize",
                &format!(
                    "end session={name} success=false kind={} detail={err:#}",
                    kind.as_str()
                ),
            );
            Err(Failure::attach(err, kind, Stage::Summarize))
        }
    }
}

fn summarize_inner(
    cfg: &Config,
    session_dir: &Path,
    on_status: impl Fn(Status),
) -> Result<PathBuf> {
    let session_dir = fs::canonicalize(session_dir)
        .with_context(|| format!("session dir {}", session_dir.display()))?;
    let info = SessionInfo::load(&session_dir)?
        .ok_or_else(|| anyhow!("no session.json in {}", session_dir.display()))?;
    let transcript_path = session_dir.join(TRANSCRIPT_FILE);
    let full_transcript_path = session_dir.join(FULL_TRANSCRIPT_FILE);
    let card_path = session_dir.join(CARD_FILE);
    if card_path.exists() && full_transcript_path.exists() {
        return Ok(card_path);
    }
    if full_transcript_path.exists() {
        let full_transcript = fs::read_to_string(&full_transcript_path)
            .with_context(|| format!("reading {}", full_transcript_path.display()))?;
        let current = fs::read_to_string(&transcript_path).unwrap_or_default();
        if current != full_transcript {
            fs::write(&transcript_path, &full_transcript)
                .with_context(|| format!("restoring {}", transcript_path.display()))?;
        }
    }
    if !transcript_path.exists() {
        bail!(
            "no {TRANSCRIPT_FILE} in {}; run `mn transcribe` first",
            session_dir.display()
        );
    }
    let transcript = fs::read_to_string(&transcript_path)
        .with_context(|| format!("reading {}", transcript_path.display()))?;

    on_status(Status::Summarizing {
        engine: Engine::Claude,
        reason: None,
    });
    let claude_prompt = build_prompt(cfg, &session_dir, &info, &transcript, Engine::Claude)?;
    let (engine, output) = match run_claude(cfg, &session_dir, &claude_prompt) {
        Ok(output) => (Engine::Claude, output),
        Err(claude_err) => {
            let (kind, _, _) = classify(&claude_err, Stage::Summarize);
            let reason = kind.as_str();
            logging::warn(
                "summarize",
                &format!("engine=claude fallback_reason={reason} detail={claude_err:#}"),
            );
            if !resolves(&cfg.codex_cli) {
                return Err(Failure::attach(
                    anyhow!("Claude failed and the Codex fallback is unavailable"),
                    kind,
                    Stage::Summarize,
                ));
            }
            logging::info(
                "summarize",
                &format!("engine=codex fallback_reason={reason} model=codex"),
            );
            on_status(Status::Summarizing {
                engine: Engine::Codex,
                reason: Some(reason.to_string()),
            });
            let codex_prompt = build_prompt(cfg, &session_dir, &info, &transcript, Engine::Codex)?;
            match run_codex(cfg, &session_dir, &codex_prompt) {
                Ok(output) => (Engine::Codex, output),
                Err(codex_err) => {
                    let codex_kind = classify(&codex_err, Stage::Summarize).0;
                    logging::error(
                        "summarize",
                        &format!(
                            "engine=codex failed kind={} detail={codex_err:#}",
                            codex_kind.as_str()
                        ),
                    );
                    let kind = if codex_kind == ErrorKind::ClaudeFailed {
                        kind
                    } else {
                        codex_kind
                    };
                    return Err(Failure::attach(
                        anyhow!("Claude and Codex could not write the notes"),
                        kind,
                        Stage::Summarize,
                    ));
                }
            }
        }
    };

    let meetings = parse_meetings(&output)
        .map_err(|err| Failure::attach(err, ErrorKind::ClaudeFailed, Stage::Summarize))?;
    logging::info(
        "summarize",
        &format!(
            "engine={} model={} meetings={}",
            engine.section(),
            if engine == Engine::Claude {
                cfg.card_model.as_str()
            } else {
                "codex"
            },
            meetings.len()
        ),
    );
    let transcripts = split_transcript(&transcript, &meetings)
        .map_err(|err| Failure::attach(err, ErrorKind::ClaudeFailed, Stage::Summarize))?;
    if meetings.len() == 1 {
        write_one_meeting(&session_dir, &info, &meetings[0].card, engine)?;
        write_completion_card(&card_path, &meetings[0].card)?;
    } else {
        write_split_meetings(
            &session_dir,
            &info,
            &transcript,
            &meetings,
            &transcripts,
            engine,
            &card_path,
        )?;
    }

    on_status(Status::Summarized {
        engine,
        card: card_path.clone(),
    });
    Ok(card_path)
}

#[derive(Debug)]
struct MeetingPart {
    card: Value,
    transcript_start: String,
    transcript_end: String,
    start_seconds: u64,
    end_seconds: u64,
}

fn parse_meetings(output: &Value) -> Result<Vec<MeetingPart>> {
    let meetings = output
        .get("meetings")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("card output has no meetings array"))?;
    if meetings.is_empty() || meetings.len() > 4 {
        bail!("card output must contain between 1 and 4 meetings");
    }

    let mut parts: Vec<MeetingPart> = Vec::with_capacity(meetings.len());
    for (index, value) in meetings.iter().enumerate() {
        let mut card = value
            .as_object()
            .cloned()
            .map(Value::Object)
            .ok_or_else(|| anyhow!("meeting {} is not an object", index + 1))?;
        let transcript_start = card
            .get("transcript_start")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("meeting {} has no transcript_start", index + 1))?
            .to_string();
        let transcript_end = card
            .get("transcript_end")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("meeting {} has no transcript_end", index + 1))?
            .to_string();
        let start_seconds = parse_transcript_offset(&transcript_start)
            .with_context(|| format!("meeting {} transcript_start", index + 1))?;
        let end_seconds = parse_transcript_offset(&transcript_end)
            .with_context(|| format!("meeting {} transcript_end", index + 1))?;
        if start_seconds > end_seconds {
            bail!(
                "meeting {} transcript range ends before it starts",
                index + 1
            );
        }
        if let Some(previous) = parts.last() {
            if start_seconds <= previous.end_seconds {
                bail!("meeting transcript ranges overlap or are out of order");
            }
        }
        let object = card
            .as_object_mut()
            .ok_or_else(|| anyhow!("meeting {} is not an object", index + 1))?;
        object.remove("transcript_start");
        object.remove("transcript_end");
        parts.push(MeetingPart {
            card,
            transcript_start,
            transcript_end,
            start_seconds,
            end_seconds,
        });
    }
    Ok(parts)
}

fn parse_transcript_offset(value: &str) -> Result<u64> {
    let bytes = value.as_bytes();
    if bytes.len() != 8 || bytes[2] != b':' || bytes[5] != b':' {
        bail!("meeting range is not HH:MM:SS");
    }
    let two_digits = |start: usize| -> Result<u64> {
        let first = bytes[start];
        let second = bytes[start + 1];
        if !first.is_ascii_digit() || !second.is_ascii_digit() {
            bail!("meeting range is not HH:MM:SS");
        }
        Ok(u64::from(first - b'0') * 10 + u64::from(second - b'0'))
    };
    let hours = two_digits(0)?;
    let minutes = two_digits(3)?;
    let seconds = two_digits(6)?;
    if minutes > 59 || seconds > 59 {
        bail!("meeting range is not a valid HH:MM:SS offset");
    }
    Ok(hours * 3600 + minutes * 60 + seconds)
}

fn transcript_timestamp(line: &str) -> Option<&str> {
    let bracketed = line.strip_prefix('[')?;
    let end = bracketed.find(']')?;
    let timestamp = &bracketed[..end];
    parse_transcript_offset(timestamp).ok().map(|_| timestamp)
}

fn split_transcript(transcript: &str, meetings: &[MeetingPart]) -> Result<Vec<String>> {
    let mut assigned_lines = vec![0usize; meetings.len()];
    let mut previous_meeting = 0usize;
    let mut timestamped_lines = 0usize;
    for line in transcript.lines() {
        let Some(timestamp) = transcript_timestamp(line) else {
            continue;
        };
        timestamped_lines += 1;
        let seconds = parse_transcript_offset(timestamp)?;
        let meeting_index = meetings
            .iter()
            .position(|meeting| seconds >= meeting.start_seconds && seconds <= meeting.end_seconds)
            .ok_or_else(|| {
                anyhow!("transcript timestamp `{timestamp}` is outside all meeting ranges")
            })?;
        if meeting_index < previous_meeting {
            bail!("transcript lines are outside the meeting range order");
        }
        previous_meeting = meeting_index;
        assigned_lines[meeting_index] += 1;
    }
    if meetings.len() > 1 && timestamped_lines == 0 {
        bail!("cannot split a transcript with no timestamped lines");
    }
    if meetings.len() > 1 && assigned_lines.contains(&0) {
        bail!("each meeting range must contain a transcript line");
    }

    meetings
        .iter()
        .map(|meeting| {
            slice_transcript(
                transcript,
                &meeting.transcript_start,
                &meeting.transcript_end,
            )
        })
        .collect()
}

fn slice_transcript(transcript: &str, start: &str, end: &str) -> Result<String> {
    let start = parse_transcript_offset(start)?;
    let end = parse_transcript_offset(end)?;
    if start > end {
        bail!("transcript range ends before it starts");
    }

    let mut output = String::new();
    let mut before_first_timestamp = true;
    for raw_line in transcript.split_inclusive('\n') {
        let line = raw_line.strip_suffix('\n').unwrap_or(raw_line);
        match transcript_timestamp(line) {
            Some(timestamp) => {
                before_first_timestamp = false;
                let seconds = parse_transcript_offset(timestamp)?;
                if seconds >= start && seconds <= end {
                    output.push_str(raw_line);
                }
            }
            None if before_first_timestamp => output.push_str(raw_line),
            None => {}
        }
    }
    Ok(output)
}

fn shift_start_time(started_at: &str, offset_seconds: u64) -> Result<String> {
    let offset = i64::try_from(offset_seconds).context("transcript offset is too large")?;
    let started = DateTime::parse_from_rfc3339(started_at)
        .with_context(|| format!("started_at `{started_at}` is not RFC3339"))?;
    Ok((started + Duration::seconds(offset)).to_rfc3339_opts(SecondsFormat::Secs, false))
}

fn meeting_session_info(info: &SessionInfo, part: &MeetingPart) -> Result<SessionInfo> {
    let duration = part.end_seconds - part.start_seconds;
    Ok(SessionInfo {
        started_at: shift_start_time(&info.started_at, part.start_seconds)?,
        ended_at: shift_start_time(&info.started_at, part.end_seconds)?,
        stop_reason: info.stop_reason,
        seconds: TrackSeconds {
            me: duration as f64,
            them: duration as f64,
        },
        audio_deleted: info.audio_deleted,
    })
}

fn write_one_meeting(
    session_dir: &Path,
    info: &SessionInfo,
    card: &Value,
    engine: Engine,
) -> Result<()> {
    let meeting = meeting_json(card, session_dir, info, engine)?;
    write_json(&session_dir.join(MEETING_FILE), &meeting)?;
    let marker = session_dir.join(PUBLISH_PENDING);
    fs::write(&marker, "").with_context(|| format!("writing {}", marker.display()))
}

fn write_split_meetings(
    session_dir: &Path,
    info: &SessionInfo,
    transcript: &str,
    meetings: &[MeetingPart],
    transcripts: &[String],
    engine: Engine,
    card_path: &Path,
) -> Result<()> {
    let notes_dir = session_dir
        .parent()
        .ok_or_else(|| anyhow!("session dir {} has no parent", session_dir.display()))?;
    let original_name = session_dir
        .file_name()
        .ok_or_else(|| anyhow!("session dir {} has no name", session_dir.display()))?
        .to_string_lossy()
        .into_owned();
    let mut occupied = fs::read_dir(notes_dir)
        .with_context(|| format!("reading notes dir {}", notes_dir.display()))?
        .map(|entry| {
            entry
                .with_context(|| format!("reading notes dir {}", notes_dir.display()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<Result<HashSet<_>>>()?;

    let original_marker = session_dir.join(PUBLISH_PENDING);
    let original_was_pending = original_marker.exists();
    let mut created_siblings = Vec::with_capacity(meetings.len() - 1);
    let result = (|| {
        for index in 1..meetings.len() {
            let sibling = create_sibling_dir(notes_dir, &original_name, &mut occupied)?;
            created_siblings.push(sibling.clone());
            write_sibling_meeting(
                &sibling,
                info,
                &meetings[index],
                &transcripts[index],
                engine,
            )
            .with_context(|| format!("writing split meeting {}", sibling.display()))?;
        }

        let full_transcript = session_dir.join(FULL_TRANSCRIPT_FILE);
        fs::write(&full_transcript, transcript)
            .with_context(|| format!("writing {}", full_transcript.display()))?;
        let transcript_path = session_dir.join(TRANSCRIPT_FILE);
        fs::write(&transcript_path, &transcripts[0])
            .with_context(|| format!("writing {}", transcript_path.display()))?;
        let first_info = meeting_session_info(info, &meetings[0])?;
        write_one_meeting(session_dir, &first_info, &meetings[0].card, engine)?;
        write_completion_card(card_path, &meetings[0].card)
    })();
    if result.is_err() {
        if !original_was_pending {
            let _ = fs::remove_file(&original_marker);
        }
        for sibling in created_siblings.iter().rev() {
            let _ = fs::remove_file(sibling.join(PUBLISH_PENDING));
            let _ = fs::remove_dir_all(sibling);
        }
    }
    result
}

fn create_sibling_dir(
    notes_dir: &Path,
    original_name: &str,
    occupied: &mut HashSet<String>,
) -> Result<PathBuf> {
    loop {
        let name = choose_sibling_dir_name(original_name, occupied)
            .ok_or_else(|| anyhow!("no free sibling session name for `{original_name}`"))?;
        let path = notes_dir.join(&name);
        match fs::create_dir(&path) {
            Ok(()) => {
                occupied.insert(name);
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                occupied.insert(name);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("creating split session {}", path.display()))
            }
        }
    }
}

fn choose_sibling_dir_name(original: &str, occupied: &HashSet<String>) -> Option<String> {
    let mut suffix = 2u64;
    loop {
        let candidate = format!("{original}-{suffix}");
        if !occupied.contains(&candidate) {
            return Some(candidate);
        }
        suffix = suffix.checked_add(1)?;
    }
}

fn write_sibling_meeting(
    sibling_dir: &Path,
    original_info: &SessionInfo,
    part: &MeetingPart,
    transcript: &str,
    engine: Engine,
) -> Result<()> {
    let info = meeting_session_info(original_info, part)?;
    info.save(sibling_dir)?;
    let transcript_path = sibling_dir.join(TRANSCRIPT_FILE);
    fs::write(&transcript_path, transcript)
        .with_context(|| format!("writing {}", transcript_path.display()))?;
    write_json(&sibling_dir.join(CARD_FILE), &part.card)?;
    let meeting = meeting_json(&part.card, sibling_dir, &info, engine)?;
    write_json(&sibling_dir.join(MEETING_FILE), &meeting)?;
    let marker = sibling_dir.join(PUBLISH_PENDING);
    fs::write(&marker, "").with_context(|| format!("writing {}", marker.display()))
}

fn run_claude(cfg: &Config, session_dir: &Path, prompt: &str) -> Result<Value> {
    let schema: Value = serde_json::from_str(SCHEMA).context("embedded card schema")?;
    let mut cmd = claude_command(cfg);
    cmd.args(["-p", "--output-format", "json", "--json-schema"])
        .arg(schema.to_string())
        .args(["--tools", ""])
        .args(["--allowedTools", CLAUDE_TOOLS])
        .args(["--model", &cfg.card_model, "--effort", &cfg.card_effort])
        .current_dir(session_dir);
    let out = run(&mut cmd, Some(prompt))
        .with_context(|| format!("could not run claude_cli {}", cfg.claude_cli.display()))?;
    logging::info(
        "summarize",
        &format!("engine=claude exit_code={} {}", out.status, reason(&out)),
    );
    if !out.status.success() {
        bail!("claude exited with {}: {}", out.status, reason(&out));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let reply: Value = serde_json::from_str(&stdout)
        .map_err(|e| anyhow!("claude output is not valid JSON ({e}; {})", reason(&out)))?;
    if reply["is_error"].as_bool().unwrap_or(false) {
        bail!("claude reported an error ({})", reason(&out));
    }
    match reply.get("structured_output") {
        Some(card) if card.is_object() => Ok(card.clone()),
        _ => bail!("claude returned no structured_output ({})", reason(&out)),
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
    let mut cmd = command(&cfg.codex_cli);
    cmd.args([
        "exec",
        "--skip-git-repo-check",
        "--sandbox",
        "read-only",
        "-C",
    ])
    .arg(session_dir)
    .arg("--output-schema")
    .arg(schema_path)
    .arg("-o")
    .arg(out_path)
    .arg("-")
    .current_dir(session_dir);
    let out = run(&mut cmd, Some(prompt))
        .with_context(|| format!("could not run codex_cli {}", cfg.codex_cli.display()))?;
    logging::info(
        "summarize",
        &format!("engine=codex exit_code={} {}", out.status, reason(&out)),
    );
    if !out.status.success() {
        bail!("codex exited with {}: {}", out.status, reason(&out));
    }
    let text = fs::read_to_string(out_path)
        .with_context(|| format!("codex wrote no {}", out_path.display()))?;
    let card: Value = serde_json::from_str(&text).map_err(|e| {
        anyhow!(
            "codex output is not valid JSON ({e}; output_bytes={})",
            text.len()
        )
    })?;
    if !card.is_object() {
        bail!(
            "codex output is not a JSON object (output_bytes={})",
            text.len()
        );
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

fn write_completion_card(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("card path {} has no parent", path.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("card path {} has no file name", path.display()))?
        .to_string_lossy();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let temporary = parent.join(format!(".{name}.tmp-{}-{nanos}", std::process::id()));
    let text = serde_json::to_string_pretty(value)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .with_context(|| format!("creating {}", temporary.display()))?;
        file.write_all(text.as_bytes())
            .with_context(|| format!("writing {}", temporary.display()))?;
        drop(file);
        if path.exists() {
            fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
        }
        fs::rename(&temporary, path)
            .with_context(|| format!("renaming {} to {}", temporary.display(), path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transcript_slice_keeps_header_and_inclusive_timestamped_lines() {
        let transcript = concat!(
            "# Transcript\n\n",
            "<!-- fake meeting -->\n",
            "[00:00:01] before\n",
            "[00:10:54] at start\n",
            "[00:20:00] at end\n",
            "[00:20:01] after\n",
        );

        let slice = slice_transcript(transcript, "00:10:54", "00:20:00").unwrap();

        assert_eq!(
            slice,
            concat!(
                "# Transcript\n\n",
                "<!-- fake meeting -->\n",
                "[00:10:54] at start\n",
                "[00:20:00] at end\n",
            )
        );
    }

    #[test]
    fn shift_start_time_preserves_its_offset_across_midnight() {
        assert_eq!(
            shift_start_time("2026-03-04T23:59:30+02:00", 90).unwrap(),
            "2026-03-05T00:01:00+02:00"
        );
    }

    #[test]
    fn sibling_name_uses_the_next_free_number() {
        let occupied = HashSet::from(["recording-2".to_string(), "recording-3".to_string()]);
        assert_eq!(
            choose_sibling_dir_name("recording", &occupied).as_deref(),
            Some("recording-4")
        );
    }
}
