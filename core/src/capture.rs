use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::exec::command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Track {
    Me,
    Them,
}

impl Track {
    pub const ALL: [Track; 2] = [Track::Me, Track::Them];

    pub fn name(self) -> &'static str {
        match self {
            Track::Me => "me",
            Track::Them => "them",
        }
    }
}

pub enum Message {
    Data(Track, Vec<u8>),
    /// The command closed stdout (it exited, or was killed).
    Ended(Track),
}

/// A running capture command. Dropping it kills the process, so no exit path leaves one behind.
pub struct CaptureProcess {
    child: Child,
}

impl CaptureProcess {
    /// Starts `argv` and forwards its stdout to `tx` from a reader thread.
    pub fn spawn(track: Track, argv: &[String], tx: Sender<Message>) -> Result<Self> {
        let Some((program, args)) = argv.split_first() else {
            bail!("capture command for {} is empty", track.name());
        };
        let mut child = command(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "could not start {} capture command `{program}`",
                    track.name()
                )
            })?;
        let mut stdout = child.stdout.take().expect("stdout is piped");
        thread::spawn(move || {
            let mut buf = vec![0u8; 8192];
            loop {
                match stdout.read(&mut buf) {
                    Ok(n) if n > 0 => {
                        if tx.send(Message::Data(track, buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                    _ => break,
                }
            }
            let _ = tx.send(Message::Ended(track));
        });
        Ok(CaptureProcess { child })
    }
}

/// How long a capture command gets to exit by itself after SIGTERM before it is killed.
#[cfg(unix)]
const STOP_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

impl Drop for CaptureProcess {
    /// Asks the command to stop first (SIGTERM on Unix), so a helper such as mn-audiotap can
    /// remove the audio devices it created, then kills it if it is still running.
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Ok(pid) = i32::try_from(self.child.id()) {
            // SAFETY: kill(2) on our own child's pid; the child is not reaped yet, so the pid
            // cannot have been reused.
            unsafe {
                libc::kill(pid, libc::SIGTERM);
            }
            let deadline = std::time::Instant::now() + STOP_GRACE;
            while std::time::Instant::now() < deadline {
                if matches!(self.child.try_wait(), Ok(Some(_))) {
                    return;
                }
                thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
