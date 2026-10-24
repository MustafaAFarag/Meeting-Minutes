use std::io::Read;
use std::process::{Child, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{bail, Context, Result};
use serde::Serialize;

use crate::exec::command;
use crate::failure::{ErrorKind, Failure, Stage};
use crate::logging;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
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
    stdout_reader: Option<thread::JoinHandle<()>>,
    stderr_reader: Option<thread::JoinHandle<(u64, Vec<u8>)>>,
    track: Track,
    finished: bool,
}

pub(crate) struct CaptureExit {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stderr_bytes: u64,
    pub stderr_tail: String,
    pub ended_naturally: bool,
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
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "could not start {} capture command `{program}`",
                    track.name()
                )
            })
            .map_err(|err| {
                let kind = if track == Track::Me {
                    ErrorKind::MicUnavailable
                } else {
                    ErrorKind::Unknown
                };
                Failure::attach(err, kind, Stage::Record)
            })?;
        let Some(mut stderr) = child.stderr.take() else {
            let _ = child.kill();
            let _ = child.wait();
            bail!("capture stderr was not captured");
        };
        let stderr_reader = thread::spawn(move || collect_tail(&mut stderr));
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            bail!("capture stdout was not captured");
        };
        let stdout_reader = thread::spawn(move || {
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
        Ok(CaptureProcess {
            child,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            track,
            finished: false,
        })
    }

    pub(crate) fn finish(mut self) -> CaptureExit {
        let result = self.stop();
        self.finished = true;
        result
    }

    fn stop(&mut self) -> CaptureExit {
        let mut status = self.child.try_wait().ok().flatten();
        let ended_naturally = status.is_some();
        if status.is_none() {
            #[cfg(unix)]
            if let Ok(pid) = i32::try_from(self.child.id()) {
                // SAFETY: kill(2) targets this unreaped child process.
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
                let deadline = std::time::Instant::now() + STOP_GRACE;
                while std::time::Instant::now() < deadline {
                    status = self.child.try_wait().ok().flatten();
                    if status.is_some() {
                        break;
                    }
                    thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            if status.is_none() {
                let _ = self.child.kill();
            }
            if status.is_none() {
                status = self.child.wait().ok();
            }
        }
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        let (stderr_bytes, stderr) = self
            .stderr_reader
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default();
        let success = status.is_some_and(|value| value.success());
        let exit_code = status.and_then(|value| value.code());
        let stderr_tail = last_lines(&String::from_utf8_lossy(&stderr), 5);
        if ended_naturally && !success {
            logging::error(
                "capture",
                &format!(
                    "track={} exit_code={} stderr_bytes={} stderr_tail={}",
                    self.track.name(),
                    exit_code.map_or_else(|| "signal".to_string(), |code| code.to_string()),
                    stderr_bytes,
                    logging::scrub(&stderr_tail)
                ),
            );
        }
        CaptureExit {
            success,
            exit_code,
            stderr_bytes,
            stderr_tail,
            ended_naturally,
        }
    }
}

/// How long a capture command gets to exit by itself after SIGTERM before it is killed.
#[cfg(unix)]
const STOP_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

impl Drop for CaptureProcess {
    /// Asks the command to stop first (SIGTERM on Unix), so a helper such as mn-audiotap can
    /// remove the audio devices it created, then kills it if it is still running.
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.stop();
        }
    }
}

fn collect_tail(pipe: &mut impl Read) -> (u64, Vec<u8>) {
    const TAIL_BYTES: usize = 8192;
    let mut count = 0u64;
    let mut tail = Vec::new();
    let mut buffer = [0u8; 2048];
    loop {
        match pipe.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(length) => {
                count = count.saturating_add(length as u64);
                tail.extend_from_slice(&buffer[..length]);
                if tail.len() > TAIL_BYTES {
                    let excess = tail.len() - TAIL_BYTES;
                    tail.drain(..excess);
                }
            }
        }
    }
    (count, tail)
}

fn last_lines(text: &str, count: usize) -> String {
    text.lines()
        .rev()
        .take(count)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}
