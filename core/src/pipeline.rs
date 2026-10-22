use std::path::Path;

use anyhow::Result;

use crate::config::Config;
use crate::publish::publish;
use crate::status::Status;
use crate::summarize::{summarize, CARD_FILE, TRANSCRIPT_FILE};
use crate::transcribe::transcribe;

/// After a recording: transcribe if there is no transcript, summarize if there is no card,
/// then publish everything pending. Stages that are already done are skipped, so a failed
/// run can simply be run again.
pub fn process(cfg: &Config, session_dir: &Path, on_status: impl Fn(Status)) -> Result<()> {
    if !session_dir.join(TRANSCRIPT_FILE).exists() {
        transcribe(cfg, session_dir, &on_status)?;
    }
    if !session_dir.join(CARD_FILE).exists() {
        summarize(cfg, session_dir, &on_status)?;
    }
    publish(cfg, &on_status)
}
