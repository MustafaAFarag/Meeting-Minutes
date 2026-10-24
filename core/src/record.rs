use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Local, SecondsFormat};

use crate::capture::{CaptureProcess, Message, Track};
use crate::config::Config;
use crate::failure::{classify, ErrorKind, Failure, Stage};
use crate::levels::{WindowMeter, FLOOR_DB, SAMPLE_RATE, WINDOWS_PER_SECOND};
use crate::logging::{self, session_name};
use crate::session::{SessionInfo, StopReason, TrackSeconds};
use crate::status::Status;

/// The GUI stops a recording by creating this file in the session dir.
const STOP_FILE: &str = "STOP";
const WINDOW: Duration = Duration::from_millis(100);
const EARLY_CAPTURE_END: Duration = Duration::from_secs(5);
const SILENT_TRACK_DB: f64 = -50.0;

#[derive(Debug, Clone, Default)]
pub struct RecordOptions {
    pub max_seconds: Option<u64>,
    /// Set this (e.g. from a Ctrl+C handler) to stop with reason `signal`.
    pub interrupt: Arc<AtomicBool>,
}

/// Writes one track's PCM to a WAV file and measures its level.
struct TrackRecorder {
    writer: hound::WavWriter<BufWriter<File>>,
    meter: WindowMeter,
    /// First byte of a sample split across two reads.
    carry: Option<u8>,
    samples: u64,
}

impl TrackRecorder {
    fn create(path: &Path) -> Result<Self> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)
            .with_context(|| format!("creating {}", path.display()))?;
        Ok(TrackRecorder {
            writer,
            meter: WindowMeter::new(),
            carry: None,
            samples: 0,
        })
    }

    fn feed(&mut self, mut bytes: &[u8]) -> Result<()> {
        let mut samples = Vec::with_capacity(bytes.len() / 2 + 1);
        if let Some(lo) = self.carry.take() {
            match bytes.split_first() {
                Some((&hi, rest)) => {
                    samples.push(i16::from_le_bytes([lo, hi]));
                    bytes = rest;
                }
                None => self.carry = Some(lo),
            }
        }
        let pairs = bytes.chunks_exact(2);
        if let [odd] = pairs.remainder() {
            self.carry = Some(*odd);
        }
        samples.extend(pairs.map(|p| i16::from_le_bytes([p[0], p[1]])));

        for &s in &samples {
            self.writer.write_sample(s)?;
        }
        self.meter.push(&samples);
        self.samples += samples.len() as u64;
        Ok(())
    }

    fn finish(self) -> Result<f64> {
        self.writer.finalize()?;
        let secs = self.samples as f64 / f64::from(SAMPLE_RATE);
        Ok((secs * 10.0).round() / 10.0)
    }
}

/// Records both tracks into a new session dir under `cfg.notes_dir` until a stop condition hits.
/// Returns the session dir.
pub fn record(cfg: &Config, opts: &RecordOptions, on_status: impl Fn(Status)) -> Result<PathBuf> {
    match record_inner(cfg, opts, on_status) {
        Ok(session) => Ok(session),
        Err(err) => {
            let kind = classify(&err, Stage::Record).0;
            logging::error(
                "record",
                &format!("success=false kind={} detail={err:#}", kind.as_str()),
            );
            Err(Failure::attach(err, kind, Stage::Record))
        }
    }
}

fn record_inner(cfg: &Config, opts: &RecordOptions, on_status: impl Fn(Status)) -> Result<PathBuf> {
    // Resolve before creating the session dir, so a pactl failure leaves nothing behind.
    let me_argv = cfg
        .capture_me_argv()
        .map_err(|err| Failure::attach(err, ErrorKind::MicUnavailable, Stage::Record))?;
    let them_argv = cfg.capture_them_argv()?;

    let started = Local::now();
    let session = create_session_dir(&cfg.notes_dir, started)?;
    let name = session_name(&session);
    logging::info("record", &format!("start session={name}"));
    let mut recorders = [
        TrackRecorder::create(&session.join("me.wav"))?,
        TrackRecorder::create(&session.join("them.wav"))?,
    ];

    // Keep `tx` alive in this function so recv_timeout never reports a disconnect.
    let (tx, rx) = mpsc::channel();
    let processes = [
        CaptureProcess::spawn(Track::Me, &me_argv, tx.clone())?,
        CaptureProcess::spawn(Track::Them, &them_argv, tx.clone())?,
    ];

    let mut ended = [false; 2];
    let mut second_peak = [FLOOR_DB; 2];
    let mut peak_db = [FLOOR_DB; 2];
    let mut ticks = 0u64;
    let start = Instant::now();
    let mut next_tick = start + WINDOW;

    let reason = loop {
        let wait = next_tick.saturating_duration_since(Instant::now());
        match rx.recv_timeout(wait) {
            Ok(Message::Data(track, bytes)) => recorders[track as usize].feed(&bytes)?,
            Ok(Message::Ended(track)) => ended[track as usize] = true,
            Err(_) => {}
        }
        if Instant::now() < next_tick {
            continue;
        }
        next_tick += WINDOW;
        ticks += 1;

        let windows = [
            recorders[0].meter.take_windows(),
            recorders[1].meter.take_windows(),
        ];
        for (second, track_windows) in second_peak.iter_mut().zip(&windows) {
            *second = track_windows.iter().copied().fold(*second, f64::max);
        }
        for (peak, track_windows) in peak_db.iter_mut().zip(&windows) {
            *peak = track_windows.iter().copied().fold(*peak, f64::max);
        }
        if ticks.is_multiple_of(WINDOWS_PER_SECOND) {
            on_status(Status::Recording {
                elapsed: ticks / WINDOWS_PER_SECOND,
                me_db: round1(second_peak[0]),
                them_db: round1(second_peak[1]),
            });
            second_peak = [FLOOR_DB; 2];
        }

        // Stop conditions are only checked on a tick, so a Ctrl+C that also killed the
        // capture commands is seen as `signal`, not as "captures ended".
        if session.join(STOP_FILE).exists() {
            break StopReason::User;
        } else if opts.interrupt.load(Ordering::SeqCst) {
            break StopReason::Signal;
        } else if opts
            .max_seconds
            .is_some_and(|max| ticks / WINDOWS_PER_SECOND >= max)
        {
            break StopReason::Max;
        } else if ended == [true; 2] {
            break StopReason::CaptureEnded;
        }
    };
    let stopped_at = start.elapsed();

    let [me_process, them_process] = processes;
    let [me_exit, them_exit] = [me_process.finish(), them_process.finish()];
    while let Ok(msg) = rx.try_recv() {
        if let Message::Data(track, bytes) = msg {
            recorders[track as usize].feed(&bytes)?;
        }
    }
    for (peak, recorder) in peak_db.iter_mut().zip(&mut recorders) {
        *peak = recorder
            .meter
            .take_windows()
            .iter()
            .copied()
            .fold(*peak, f64::max);
    }
    let ended_at = Local::now();
    let duration = start.elapsed();
    let [me, them] = recorders;
    let info = SessionInfo {
        started_at: rfc3339(started),
        ended_at: rfc3339(ended_at),
        stop_reason: reason,
        seconds: TrackSeconds {
            me: me.finish()?,
            them: them.finish()?,
        },
        audio_deleted: false,
    };
    info.save(&session)?;

    let silent = peak_db.map(|peak| peak <= SILENT_TRACK_DB);
    logging::info(
        "record",
        &format!(
            "stop session={name} duration_ms={} reason={} me_seconds={:.1} them_seconds={:.1} me_peak_db={:.1} them_peak_db={:.1} me_exit_code={} them_exit_code={} me_capture_ended={} them_capture_ended={}",
            duration.as_millis(),
            stop_reason_name(reason),
            info.seconds.me,
            info.seconds.them,
            round1(peak_db[0]),
            round1(peak_db[1]),
            me_exit.exit_code.map_or_else(|| "signal".to_string(), |code| code.to_string()),
            them_exit.exit_code.map_or_else(|| "signal".to_string(), |code| code.to_string()),
            me_exit.ended_naturally,
            them_exit.ended_naturally,
        ),
    );
    let me_stderr = me_exit.stderr_tail.to_lowercase();
    let mic_problem = me_exit.ended_naturally
        && !me_exit.success
        && (me_stderr.contains("microphone")
            || me_stderr.contains("input device")
            || me_stderr.contains("permission")
            || me_stderr.contains("audio stream"));
    if mic_problem {
        return Err(Failure::attach(
            anyhow!(
                "microphone capture failed (stderr_bytes={})",
                me_exit.stderr_bytes
            ),
            ErrorKind::MicUnavailable,
            Stage::Record,
        ));
    }
    if reason == StopReason::CaptureEnded && stopped_at <= EARLY_CAPTURE_END {
        return Err(Failure::attach(
            anyhow!(
                "capture ended within the first seconds (stderr_bytes={})",
                me_exit.stderr_bytes
            ),
            ErrorKind::NoSound,
            Stage::Record,
        ));
    }
    if silent == [true, true] {
        return Err(Failure::attach(
            anyhow!("both audio tracks were silent"),
            ErrorKind::NoSound,
            Stage::Record,
        ));
    }
    for (track, is_silent, peak) in [
        (Track::Me, silent[0], peak_db[0]),
        (Track::Them, silent[1], peak_db[1]),
    ] {
        if is_silent {
            logging::warn(
                "record",
                &format!("track={} silent peak_db={:.1}", track.name(), round1(peak)),
            );
        }
    }

    on_status(Status::Stopped {
        session: session.clone(),
        reason,
    });
    Ok(session)
}

fn stop_reason_name(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Silence => "silence",
        StopReason::User => "user",
        StopReason::Max => "max",
        StopReason::Signal => "signal",
        StopReason::CaptureEnded => "capture_ended",
    }
}

/// `<notes_dir>/<YYYY-MM-DD_HHMM>`, with a numeric suffix if that minute is already taken.
fn create_session_dir(notes_dir: &Path, started: DateTime<Local>) -> Result<PathBuf> {
    fs::create_dir_all(notes_dir)
        .with_context(|| format!("creating notes dir {}", notes_dir.display()))?;
    let base = started.format("%Y-%m-%d_%H%M").to_string();
    for n in 1.. {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        let dir = notes_dir.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(e).with_context(|| format!("creating session dir {}", dir.display()))
            }
        }
    }
    unreachable!()
}

fn rfc3339(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn round1(db: f64) -> f64 {
    (db * 10.0).round() / 10.0
}
