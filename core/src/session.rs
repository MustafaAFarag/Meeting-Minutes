use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const SESSION_FILE: &str = "session.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StopReason {
    /// Only in sessions recorded before auto-stop was removed; never produced now.
    Silence,
    User,
    Max,
    Signal,
    /// Both capture commands exited, so the recording broke.
    #[serde(rename = "capture_ended")]
    CaptureEnded,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackSeconds {
    pub me: f64,
    pub them: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// RFC3339 local time with offset.
    pub started_at: String,
    pub ended_at: String,
    pub stop_reason: StopReason,
    pub seconds: TrackSeconds,
    #[serde(default)]
    pub audio_deleted: bool,
}

impl SessionInfo {
    /// `Ok(None)` when the session has no session.json.
    pub fn load(session_dir: &Path) -> Result<Option<Self>> {
        let path = session_dir.join(SESSION_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let info =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(info))
    }

    pub fn save(&self, session_dir: &Path) -> Result<()> {
        let path = session_dir.join(SESSION_FILE);
        let text = serde_json::to_string_pretty(self)?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}
