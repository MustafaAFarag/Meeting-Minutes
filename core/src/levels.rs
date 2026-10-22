use std::collections::VecDeque;
use std::path::Path;

use anyhow::{Context, Result};

pub const SAMPLE_RATE: u32 = 16_000;
pub const WINDOW_SAMPLES: usize = 1_600; // 100 ms
pub const WINDOWS_PER_SECOND: u64 = 10;
/// Level reported for digital silence (log10(0) would be -inf, which JSON cannot carry).
pub const FLOOR_DB: f64 = -100.0;

/// The noise floor is estimated from this many recent windows (60 s).
const NOISE_HISTORY_WINDOWS: usize = 60 * WINDOWS_PER_SECOND as usize;
/// The noise floor is this percentile of the recent window levels.
const NOISE_PERCENTILE: usize = 10;

/// RMS level in dBFS. A full-scale sine is about -3 dB.
pub fn rms_db(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return FLOOR_DB;
    }
    let sum: f64 = samples
        .iter()
        .map(|&s| {
            let v = f64::from(s) / 32768.0;
            v * v
        })
        .sum();
    if sum <= 0.0 {
        return FLOOR_DB;
    }
    (10.0 * (sum / samples.len() as f64).log10()).max(FLOOR_DB)
}

/// Level of the loudest 100 ms window in a WAV file, read in chunks.
pub fn wav_peak_db(path: &Path) -> Result<f64> {
    const CHUNK_SAMPLES: usize = WINDOW_SAMPLES * 100;
    let mut reader =
        hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut samples = reader.samples::<i16>();
    let mut meter = WindowMeter::new();
    let mut chunk = Vec::with_capacity(CHUNK_SAMPLES);
    let mut peak = FLOOR_DB;
    loop {
        chunk.clear();
        for sample in samples.by_ref().take(CHUNK_SAMPLES) {
            chunk.push(sample.with_context(|| format!("reading {}", path.display()))?);
        }
        if chunk.is_empty() {
            return Ok(peak);
        }
        meter.push(&chunk);
        for db in meter.take_windows() {
            peak = peak.max(db);
        }
    }
}

/// Cuts a PCM stream into 100 ms windows and keeps the level of each complete one until taken.
pub struct WindowMeter {
    pending: Vec<i16>,
    windows: Vec<f64>,
}

impl WindowMeter {
    pub fn new() -> Self {
        WindowMeter {
            pending: Vec::new(),
            windows: Vec::new(),
        }
    }

    pub fn push(&mut self, samples: &[i16]) {
        self.pending.extend_from_slice(samples);
        let complete = self.pending.len() / WINDOW_SAMPLES * WINDOW_SAMPLES;
        for window in self.pending[..complete].chunks(WINDOW_SAMPLES) {
            self.windows.push(rms_db(window));
        }
        self.pending.drain(..complete);
    }

    /// Levels of the windows completed since the last call.
    pub fn take_windows(&mut self) -> Vec<f64> {
        std::mem::take(&mut self.windows)
    }
}

impl Default for WindowMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Decides per mic window whether it carries sound, relative to the mic's own noise floor.
/// A room mic sits above any fixed threshold, so "sound" must stand out from the recent noise.
struct MicDetector {
    threshold_db: f64,
    margin_db: f64,
    max_floor_db: f64,
    history: VecDeque<f64>,
}

impl MicDetector {
    fn new(threshold_db: f64, margin_db: f64, max_floor_db: f64) -> Self {
        MicDetector {
            threshold_db,
            margin_db,
            max_floor_db,
            history: VecDeque::new(),
        }
    }

    /// Records the window, then checks it.
    fn is_sound(&mut self, level_db: f64) -> bool {
        if self.history.len() == NOISE_HISTORY_WINDOWS {
            self.history.pop_front();
        }
        self.history.push_back(level_db);
        // The clamp keeps speech from becoming the floor when the history has no quiet stretch.
        let floor = self.noise_floor().min(self.max_floor_db);
        level_db >= self.threshold_db.max(floor + self.margin_db)
    }

    fn noise_floor(&self) -> f64 {
        let mut levels: Vec<f64> = self.history.iter().copied().collect();
        let rank = (levels.len() - 1) * NOISE_PERCENTILE / 100;
        *levels.select_nth_unstable_by(rank, f64::total_cmp).1
    }
}

/// Auto-stop state machine, fed once per 100 ms tick with the windows each track completed in it.
/// The timer only starts once either track has made sound, so a call that has not begun yet
/// does not stop the recording.
///
/// "Me" is the mic and uses the adaptive `MicDetector`. "Them" is digital system audio that
/// drops to about -100 dB between speech, so it only uses the absolute threshold.
pub struct SilenceTracker {
    me: MicDetector,
    them_threshold_db: f64,
    limit_ticks: u64,
    heard_sound: bool,
    quiet_ticks: u64,
}

impl SilenceTracker {
    pub fn new(
        threshold_db: f64,
        speech_margin_db: f64,
        max_noise_floor_db: f64,
        silence_seconds: u64,
    ) -> Self {
        SilenceTracker {
            me: MicDetector::new(threshold_db, speech_margin_db, max_noise_floor_db),
            them_threshold_db: threshold_db,
            limit_ticks: silence_seconds * WINDOWS_PER_SECOND,
            heard_sound: false,
            quiet_ticks: 0,
        }
    }

    /// A tick with no windows (a stalled stream) counts as quiet.
    pub fn push(&mut self, me_windows: &[f64], them_windows: &[f64]) {
        let mut sound = false;
        // every mic window must reach its detector, so no short-circuiting
        for &db in me_windows {
            sound |= self.me.is_sound(db);
        }
        sound |= them_windows.iter().any(|&db| db >= self.them_threshold_db);
        if sound {
            self.heard_sound = true;
            self.quiet_ticks = 0;
        } else if self.heard_sound {
            self.quiet_ticks += 1;
        }
    }

    pub fn silent_for(&self) -> u64 {
        self.quiet_ticks / WINDOWS_PER_SECOND
    }

    pub fn should_stop(&self) -> bool {
        self.heard_sound && self.quiet_ticks >= self.limit_ticks
    }
}
