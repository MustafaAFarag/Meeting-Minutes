use std::path::Path;

use anyhow::{Context, Result};

pub const SAMPLE_RATE: u32 = 16_000;
pub const WINDOW_SAMPLES: usize = 1_600; // 100 ms
pub const WINDOWS_PER_SECOND: u64 = 10;
/// Level reported for digital silence (log10(0) would be -inf, which JSON cannot carry).
pub const FLOOR_DB: f64 = -100.0;

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
