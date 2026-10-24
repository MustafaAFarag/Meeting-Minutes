use std::error::Error;
use std::fmt;

use anyhow::Error as AnyhowError;
use serde::{Deserialize, Serialize};

/// The kind of failure shown to a PM by the app.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    NoSound,
    MicUnavailable,
    Whisper,
    ClaudeSignedOut,
    ClaudeFailed,
    Network,
    Publish,
    Config,
    #[default]
    Unknown,
}

impl ErrorKind {
    pub const fn as_str(self) -> &'static str {
        self.name()
    }

    pub const fn message(self) -> &'static str {
        match self {
            Self::NoSound => "No sound was recorded.",
            Self::MicUnavailable => "The microphone could not be opened.",
            Self::Whisper => "The speech-to-text step failed.",
            Self::ClaudeSignedOut => "Claude is signed out, so the notes could not be written.",
            Self::ClaudeFailed => "Claude could not write the notes.",
            Self::Network => "No internet connection.",
            Self::Publish => "Your notes were written but could not be added to My Meetings.",
            Self::Config => "Meeting Notes is not set up correctly.",
            Self::Unknown => "Something went wrong.",
        }
    }

    pub const fn hint(self) -> &'static str {
        match self {
            Self::NoSound => "Check that Meeting Notes may use the microphone (and on a Mac, Screen & System Audio Recording), then try a short test.",
            Self::MicUnavailable => "Check your microphone is connected and allowed for Meeting Notes.",
            Self::Whisper => "Try again. If it fails again, use Report a problem.",
            Self::ClaudeSignedOut => "Open Claude and sign in, then click Try again.",
            Self::ClaudeFailed => "Try again in a few minutes. If it keeps failing, use Report a problem.",
            Self::Network => "Connect to the internet, then click Try again.",
            Self::Publish => "Click Try again. Your notes are saved on this computer.",
            Self::Config => "Ask Claude to repair the install, or use Report a problem.",
            Self::Unknown => "Click Try again. If it happens again, use Report a problem.",
        }
    }
}

/// The core stage where a failure happened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Record,
    Transcribe,
    Summarize,
    Publish,
    InitPage,
    #[default]
    Other,
}

impl Stage {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Transcribe => "transcribe",
            Self::Summarize => "summarize",
            Self::Publish => "publish",
            Self::InitPage => "init_page",
            Self::Other => "other",
        }
    }
}

/// Technical detail attached to an anyhow error for app classification.
#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: ErrorKind,
    pub stage: Stage,
    pub detail: String,
}

impl Failure {
    /// Adds a typed failure while retaining the source error chain.
    pub fn attach(err: AnyhowError, kind: ErrorKind, stage: Stage) -> AnyhowError {
        if err.downcast_ref::<Self>().is_some() {
            return err;
        }
        let detail = crate::logging::scrub(&format!("{err:#}"));
        err.context(Self {
            kind,
            stage,
            detail,
        })
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} failure: {}", self.kind.name(), self.detail)
    }
}

impl Error for Failure {}

impl ErrorKind {
    const fn name(self) -> &'static str {
        match self {
            Self::NoSound => "no_sound",
            Self::MicUnavailable => "mic_unavailable",
            Self::Whisper => "whisper",
            Self::ClaudeSignedOut => "claude_signed_out",
            Self::ClaudeFailed => "claude_failed",
            Self::Network => "network",
            Self::Publish => "publish",
            Self::Config => "config",
            Self::Unknown => "unknown",
        }
    }
}

/// Converts technical errors to the fixed plain message and hint used by the app.
pub fn classify(err: &AnyhowError, stage: Stage) -> (ErrorKind, &'static str, &'static str) {
    if let Some(failure) = err.downcast_ref::<Failure>() {
        return (failure.kind, failure.kind.message(), failure.kind.hint());
    }

    let detail = format!("{err:#}").to_lowercase();
    let kind = if detail.contains("no sound")
        || detail.contains("both tracks are silent")
        || detail.contains("audio tracks were silent")
    {
        ErrorKind::NoSound
    } else if stage == Stage::Summarize
        && (detail.contains("not logged in")
            || detail.contains("logged out")
            || detail.contains("signed out")
            || detail.contains("authentication required")
            || detail.contains("authentication")
            || detail.contains("login required")
            || detail.contains("not authenticated")
            || detail.contains("auth failed")
            || detail.contains("auth error")
            || detail.contains("please sign in")
            || detail.contains("claude auth login")
            || detail.contains("token expired")
            || detail.contains("session expired")
            || detail.contains("unauthorized"))
    {
        ErrorKind::ClaudeSignedOut
    } else if detail.contains("microphone")
        || detail.contains("input device")
        || detail.contains("no default input")
    {
        ErrorKind::MicUnavailable
    } else if detail.contains("network")
        || detail.contains("dns")
        || detail.contains("connection refused")
        || detail.contains("connection reset")
        || detail.contains("connection error")
        || detail.contains("could not connect")
        || detail.contains("could not resolve host")
        || detail.contains("failed to fetch")
        || detail.contains("fetch failed")
        || detail.contains("econnreset")
        || detail.contains("eai_again")
        || detail.contains("no internet")
        || detail.contains("timed out")
    {
        ErrorKind::Network
    } else if stage == Stage::Transcribe || detail.contains("whisper") {
        ErrorKind::Whisper
    } else if matches!(stage, Stage::Publish | Stage::InitPage) {
        ErrorKind::Publish
    } else if detail.contains("config") || detail.contains("settings") {
        ErrorKind::Config
    } else if stage == Stage::Summarize && (detail.contains("claude") || detail.contains("codex")) {
        ErrorKind::ClaudeFailed
    } else {
        ErrorKind::Unknown
    };
    (kind, kind.message(), kind.hint())
}
