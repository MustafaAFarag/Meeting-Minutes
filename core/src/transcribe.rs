use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use chrono::DateTime;
use serde::{Deserialize, Serialize};

use crate::capture::Track;
use crate::config::Config;
use crate::levels::wav_peak_db;
use crate::session::SessionInfo;
use crate::status::Status;

/// A track whose loudest 100 ms window is at or below this is skipped, not sent to whisper.
const SILENT_TRACK_DB: f64 = -50.0;

/// Whisper invents these on silence or noise.
const HALLUCINATIONS: [&str; 6] = [
    "you",
    "thank you.",
    "thanks for watching!",
    "[blank_audio]",
    "[music]",
    "(silence)",
];

/// One character repeated this many times in a row is a token loop.
const MAX_CHAR_RUN: usize = 8;
/// Longer than this, distinct characters must be at least `VARIETY_PERCENT` of the length,
/// counted over at most `VARIETY_LEN_CAP` characters.
const VARIETY_CHECK_MIN_LEN: usize = 20;
const VARIETY_PERCENT: usize = 15;
const VARIETY_LEN_CAP: usize = 100;

/// Same speaker, gap at most this long: one line.
const JOIN_GAP_MS: u64 = 2_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: Track,
    pub text: String,
}

#[derive(Deserialize)]
struct WhisperOutput {
    transcription: Vec<WhisperSegment>,
}

#[derive(Deserialize)]
struct WhisperSegment {
    offsets: Offsets,
    text: String,
}

#[derive(Deserialize)]
struct Offsets {
    from: u64,
    to: u64,
}

pub fn parse_whisper_json(json: &str, speaker: Track) -> Result<Vec<Segment>> {
    let output: WhisperOutput =
        serde_json::from_str(json).context("not whisper.cpp JSON output")?;
    Ok(output
        .transcription
        .into_iter()
        .map(|s| Segment {
            start_ms: s.offsets.from,
            end_ms: s.offsets.to,
            speaker,
            text: s.text.trim().to_string(),
        })
        .collect())
}

pub fn is_hallucination(text: &str) -> bool {
    let text = text.trim().to_lowercase();
    text.is_empty() || HALLUCINATIONS.contains(&text.as_str())
}

/// Wrong-language detection plus a token loop gives text like "වවවවවවවවවව": one character
/// repeated, or a long line with almost no variety.
pub fn is_garbage(text: &str) -> bool {
    let chars: Vec<char> = text.trim().chars().collect();
    let repeated = chars
        .chunk_by(|a, b| a == b)
        .any(|run| run.len() >= MAX_CHAR_RUN);
    let distinct = chars.iter().collect::<HashSet<_>>().len();
    // Capped length: a plain 15%-of-length rule would drop long normal sentences, since a few
    // hundred characters of real text only use about 30 distinct ones.
    let low_variety = chars.len() > VARIETY_CHECK_MIN_LEN
        && distinct * 100 < VARIETY_PERCENT * chars.len().min(VARIETY_LEN_CAP);
    repeated || low_variety
}

/// One timeline sorted by start time; neighbours from the same speaker close together become one line.
pub fn merge(mut segments: Vec<Segment>) -> Vec<Segment> {
    segments.sort_by_key(|s| s.start_ms);
    let mut merged: Vec<Segment> = Vec::new();
    for seg in segments {
        match merged.last_mut() {
            Some(prev)
                if prev.speaker == seg.speaker
                    && seg.start_ms.saturating_sub(prev.end_ms) <= JOIN_GAP_MS =>
            {
                prev.text.push(' ');
                prev.text.push_str(&seg.text);
                prev.end_ms = prev.end_ms.max(seg.end_ms);
            }
            _ => merged.push(seg),
        }
    }
    merged
}

pub fn format_transcript(header: &str, segments: &[Segment]) -> String {
    let mut out = format!("{header}\n");
    if !segments.is_empty() {
        out.push('\n');
    }
    for seg in segments {
        let speaker = match seg.speaker {
            Track::Me => "Me",
            Track::Them => "Them",
        };
        out.push_str(&format!(
            "[{}] **{speaker}:** {}\n",
            clock(seg.start_ms),
            seg.text
        ));
    }
    out
}

fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
}

fn header(info: Option<&SessionInfo>, session_dir: &Path) -> String {
    let times = info.and_then(|i| {
        let start = DateTime::parse_from_rfc3339(&i.started_at).ok()?;
        let end = DateTime::parse_from_rfc3339(&i.ended_at).ok()?;
        Some(format!(
            "{} {}–{}",
            start.format("%Y-%m-%d"),
            start.format("%H:%M"),
            end.format("%H:%M")
        ))
    });
    let label = times.unwrap_or_else(|| {
        session_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    format!("# Transcript {label}")
}

/// Transcribes me.wav and them.wav, writes transcript.md and transcript.json, then deletes the audio.
/// On any failure the audio is kept so the session can be retried.
pub fn transcribe(cfg: &Config, session_dir: &Path, on_status: impl Fn(Status)) -> Result<PathBuf> {
    let model = cfg.whisper_model.as_deref().ok_or_else(|| {
        anyhow!(
            "whisper_model is not set; add `whisper_model = \"/path/to/ggml-small.bin\"` to the config file (see `mn config`)"
        )
    })?;
    let wavs: Vec<(Track, PathBuf)> = Track::ALL
        .into_iter()
        .map(|t| (t, session_dir.join(format!("{}.wav", t.name()))))
        .filter(|(_, wav)| wav.exists())
        .collect();
    if wavs.is_empty() {
        bail!("no me.wav or them.wav in {}", session_dir.display());
    }
    let info = SessionInfo::load(session_dir)?;

    let mut segments = Vec::new();
    for (track, wav) in &wavs {
        // Whole-file RMS would skip a quiet speaker in a long call, so look at single windows.
        if wav_peak_db(wav)? <= SILENT_TRACK_DB {
            continue;
        }
        on_status(Status::Transcribing { track: *track });
        segments.extend(run_whisper(cfg, model, session_dir, *track, wav)?);
    }
    let segments = merge(segments);

    let json_path = session_dir.join("transcript.json");
    fs::write(&json_path, serde_json::to_string_pretty(&segments)?)
        .with_context(|| format!("writing {}", json_path.display()))?;
    let transcript = session_dir.join("transcript.md");
    fs::write(
        &transcript,
        format_transcript(&header(info.as_ref(), session_dir), &segments),
    )
    .with_context(|| format!("writing {}", transcript.display()))?;

    for (track, wav) in &wavs {
        remove(wav)?;
        remove(&whisper_json_path(session_dir, *track))?;
    }
    if let Some(mut info) = info {
        info.audio_deleted = true;
        info.save(session_dir)?;
    }

    on_status(Status::Transcribed {
        transcript: transcript.clone(),
    });
    Ok(transcript)
}

fn whisper_json_path(session_dir: &Path, track: Track) -> PathBuf {
    session_dir.join(format!("{}.json", track.name()))
}

fn run_whisper(
    cfg: &Config,
    model: &Path,
    session_dir: &Path,
    track: Track,
    wav: &Path,
) -> Result<Vec<Segment>> {
    let mut cmd = Command::new(&cfg.whisper_cli);
    cmd.arg("-m")
        .arg(model)
        .arg("-f")
        .arg(wav)
        .args(["-l", &cfg.language, "-oj", "-of"])
        .arg(session_dir.join(track.name()))
        .arg("-np")
        .arg("-t")
        .arg(cfg.threads.max(1).to_string());
    if let Some(vad_model) = &cfg.vad_model {
        cmd.args(["--vad", "-vm"]).arg(vad_model);
    }
    if let Some(prompt) = &cfg.vocab_prompt {
        cmd.arg("--prompt").arg(prompt);
    }
    // output() captures stdout too, so whisper cannot mix into our JSON status lines
    let out = cmd
        .output()
        .with_context(|| format!("could not run whisper_cli {}", cfg.whisper_cli.display()))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(5).collect();
        let tail: Vec<&str> = tail.into_iter().rev().collect();
        bail!(
            "whisper failed on {}.wav ({}): {}",
            track.name(),
            out.status,
            tail.join("\n")
        );
    }

    let path = whisper_json_path(session_dir, track);
    let json = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let mut segments =
        parse_whisper_json(&json, track).with_context(|| format!("parsing {}", path.display()))?;
    segments.retain(|s| !is_hallucination(&s.text) && !is_garbage(&s.text));
    Ok(segments)
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| format!("deleting {}", path.display()))
        }
        _ => Ok(()),
    }
}
