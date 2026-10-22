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

impl Drop for CaptureProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
