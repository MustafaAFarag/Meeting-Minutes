use std::path::PathBuf;

use serde::Serialize;

use crate::capture::Track;
use crate::session::StopReason;

/// One JSON line on stdout. The GUI parses these.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    Recording {
        elapsed: u64,
        me_db: f64,
        them_db: f64,
        silent_for: u64,
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
    Error {
        message: String,
    },
}
