use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::capture::Track;
use crate::failure::{ErrorKind, Stage};
use crate::session::StopReason;
use crate::summarize::Engine;

/// One JSON line on stdout. The GUI parses these.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    Recording {
        elapsed: u64,
        me_db: f64,
        them_db: f64,
    },
    Stopped {
        session: PathBuf,
        reason: StopReason,
    },
    Transcribing {
        track: Track,
    },
    Transcribed {
        transcript: PathBuf,
    },
    Summarizing {
        engine: Engine,
        /// Why Claude was given up on; only on the Codex line.
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Summarized {
        engine: Engine,
        card: PathBuf,
    },
    Publishing {
        meetings: usize,
    },
    Published {
        url: String,
    },
    Error {
        message: String,
        #[serde(default)]
        kind: ErrorKind,
        #[serde(default)]
        stage: Stage,
        #[serde(default = "default_error_hint")]
        hint: String,
    },
}

fn default_error_hint() -> String {
    ErrorKind::Unknown.hint().to_string()
}
