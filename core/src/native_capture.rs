//! The built-in capture command (`mn capture mic|system`) for Windows and macOS: records with
//! cpal and writes raw s16le 16 kHz mono PCM, the same stream `parec` gives on Linux.
//! The conversion and the write loop are plain code so they are tested on any OS; only
//! `capture` below touches cpal.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use std::io::{self, Write};
use std::str::FromStr;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

use anyhow::{bail, Context, Result};

use crate::levels::{SAMPLE_RATE, WINDOW_SAMPLES};

/// How long the pump waits for audio before it counts the wait as silence (see `pump`).
const CHUNK_WAIT: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource {
    Mic,
    System,
}

impl FromStr for CaptureSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "mic" => Ok(CaptureSource::Mic),
            "system" => Ok(CaptureSource::System),
            other => Err(format!(
                "unknown capture source `{other}` (use mic or system)"
            )),
        }
    }
}

/// Records `source` and writes s16le 16 kHz mono PCM to `out` in chunks of 100 ms.
/// Runs until the write fails because the reader closed the pipe (that is `Ok`) or the
/// process is killed.
#[cfg(any(windows, target_os = "macos"))]
pub fn run_capture(source: CaptureSource, out: impl Write) -> Result<()> {
    capture(source, out)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn run_capture(_source: CaptureSource, _out: impl Write) -> Result<()> {
    bail!(
        "`mn capture` is built only for Windows and macOS; on Linux the default capture commands use pactl and parec"
    )
}

/// What the cpal callbacks send to the thread that writes.
enum Event {
    Samples(Vec<i16>),
    Failed(String),
}

#[cfg(any(windows, target_os = "macos"))]
fn capture(source: CaptureSource, mut out: impl Write) -> Result<()> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::sync::mpsc;

    let host = cpal::default_host();
    let (device, supported) = match source {
        CaptureSource::Mic => {
            let device = host
                .default_input_device()
                .context("no default input device (microphone)")?;
            let config = device
                .default_input_config()
                .context("could not read the microphone format")?;
            (device, config)
        }
        CaptureSource::System => system_device(&host)?,
    };
    let name = device.name().unwrap_or_else(|_| "unknown device".into());
    let format = supported.sample_format();
    let config = supported.config();
    let mut converter = Converter::new(config.channels, config.sample_rate.0);

    let (tx, rx) = mpsc::channel();
    let failed = tx.clone();
    let stream = device
        .build_input_stream_raw(
            &config,
            format,
            move |data, _| {
                let mut pcm = Vec::new();
                converter.convert(&samples_f32(data), &mut pcm);
                // The receiver is gone once the writer has stopped.
                let _ = tx.send(Event::Samples(pcm));
            },
            move |err| {
                let _ = failed.send(Event::Failed(err.to_string()));
            },
            None,
        )
        .with_context(|| format!("could not open an audio stream on `{name}`"))?;
    stream
        .play()
        .with_context(|| format!("could not start the audio stream on `{name}`"))?;

    // A loopback stream delivers nothing while nothing plays, so the system track gets
    // silence for those gaps; otherwise it would run shorter than the mic track.
    pump(&rx, &mut out, source == CaptureSource::System, CHUNK_WAIT)
}

/// Loopback records what the default output device plays. cpal has no separate loopback API:
/// an input stream built on an *output* device is a WASAPI loopback. The format comes from
/// `default_output_config`; `default_input_config` is an error on an output device.
#[cfg(windows)]
fn system_device(host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    use cpal::traits::{DeviceTrait, HostTrait};

    let device = host
        .default_output_device()
        .context("no default output device to record the system audio from")?;
    let config = device
        .default_output_config()
        .context("could not read the output device format")?;
    Ok((device, config))
}

#[cfg(target_os = "macos")]
fn system_device(_host: &cpal::Host) -> Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    bail!("macOS system audio comes from the `mn-audiotap` helper, not from `capture system`")
}

/// The callback's buffer as f32, whatever format the device uses.
#[cfg(any(windows, target_os = "macos"))]
fn samples_f32(data: &cpal::Data) -> Vec<f32> {
    use cpal::SampleFormat;

    match data.sample_format() {
        SampleFormat::I8 => typed::<i8>(data),
        SampleFormat::I16 => typed::<i16>(data),
        SampleFormat::I32 => typed::<i32>(data),
        SampleFormat::I64 => typed::<i64>(data),
        SampleFormat::U8 => typed::<u8>(data),
        SampleFormat::U16 => typed::<u16>(data),
        SampleFormat::U32 => typed::<u32>(data),
        SampleFormat::U64 => typed::<u64>(data),
        SampleFormat::F32 => typed::<f32>(data),
        SampleFormat::F64 => typed::<f64>(data),
        // A format added by a later cpal: drop the buffer instead of guessing.
        _ => Vec::new(),
    }
}

#[cfg(any(windows, target_os = "macos"))]
fn typed<T: cpal::SizedSample>(data: &cpal::Data) -> Vec<f32>
where
    f32: cpal::FromSample<T>,
{
    data.as_slice::<T>()
        .unwrap_or_default()
        .iter()
        .map(|&s| s.to_sample::<f32>())
        .collect()
}

/// Turns interleaved f32 audio at any rate and channel count into 16 kHz mono i16.
/// It keeps its place between calls, so the stream can be fed callback by callback.
struct Converter {
    channels: usize,
    /// Input samples per output sample.
    step: f64,
    /// Where the next output sample falls, in input samples counted from the start of
    /// `prev` (when there is one) or of the next chunk.
    pos: f64,
    /// The last mono sample of the previous chunk, the left point of the next interpolation.
    prev: Option<f32>,
}

impl Converter {
    fn new(channels: u16, rate: u32) -> Self {
        Converter {
            channels: usize::from(channels).max(1),
            step: f64::from(rate.max(1)) / f64::from(SAMPLE_RATE),
            pos: 0.0,
            prev: None,
        }
    }

    /// Appends the converted samples to `out`. Linear interpolation, no low-pass filter.
    fn convert(&mut self, interleaved: &[f32], out: &mut Vec<i16>) {
        let mut line: Vec<f32> = self.prev.into_iter().collect();
        line.extend(downmix(interleaved, self.channels));
        let Some(&last) = line.last() else {
            return;
        };
        while (self.pos as usize) + 1 < line.len() {
            let i = self.pos as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(to_i16(line[i] + (line[i + 1] - line[i]) * frac));
            self.pos += self.step;
        }
        self.pos -= (line.len() - 1) as f64;
        self.prev = Some(last);
    }
}

/// The average of each frame's channels. A trailing partial frame is dropped.
fn downmix(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return interleaved.to_vec();
    }
    interleaved
        .chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

/// Writes every full 100 ms chunk of `pending` to `out` and flushes, keeping the rest.
fn write_chunks(pending: &mut Vec<i16>, out: &mut impl Write) -> io::Result<()> {
    let full = pending.len() / WINDOW_SAMPLES * WINDOW_SAMPLES;
    if full == 0 {
        return Ok(());
    }
    let bytes: Vec<u8> = pending[..full]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    pending.drain(..full);
    out.write_all(&bytes)?;
    out.flush()
}

/// Collects the callbacks' samples and writes them out in 100 ms chunks until the writer
/// fails. A closed pipe is the normal way to stop and returns `Ok`.
/// With `fill_silence`, a `wait` with no audio adds 100 ms of silence.
fn pump(
    rx: &Receiver<Event>,
    out: &mut impl Write,
    fill_silence: bool,
    wait: Duration,
) -> Result<()> {
    let mut pending: Vec<i16> = Vec::new();
    loop {
        match rx.recv_timeout(wait) {
            Ok(Event::Samples(samples)) => pending.extend(samples),
            Ok(Event::Failed(message)) => bail!("the audio stream failed: {message}"),
            Err(RecvTimeoutError::Timeout) => {
                if fill_silence {
                    pending.resize(pending.len() + WINDOW_SAMPLES, 0);
                }
            }
            Err(RecvTimeoutError::Disconnected) => bail!("the audio stream ended"),
        }
        match write_chunks(&mut pending, out) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(e) => return Err(e).context("writing audio to stdout"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn convert_all(channels: u16, rate: u32, chunks: &[&[f32]]) -> Vec<i16> {
        let mut converter = Converter::new(channels, rate);
        let mut out = Vec::new();
        for chunk in chunks {
            converter.convert(chunk, &mut out);
        }
        out
    }

    #[test]
    fn stereo_48k_f32_becomes_mono_16k_i16() {
        // 480 frames (10 ms) of a constant: left 0.5, right 0.25, so the mono value is 0.375.
        let input: Vec<f32> = (0..480).flat_map(|_| [0.5, 0.25]).collect();
        let out = convert_all(2, 48_000, &[&input]);
        // 10 ms at 16 kHz is 160 samples; the last one waits for the next callback.
        assert_eq!(out.len(), 160);
        assert!(out.iter().all(|&s| s == to_i16(0.375)), "{out:?}");
    }

    #[test]
    fn a_44_1k_stream_gives_16k_samples_per_second() {
        let one_second = vec![0.1f32; 44_100];
        let out = convert_all(1, 44_100, &[&one_second, &one_second]);
        // Two seconds in, 32000 samples out, within the sample still waiting for its neighbour.
        assert!(
            (31_998..=32_000).contains(&out.len()),
            "got {} samples",
            out.len()
        );
    }

    #[test]
    fn silence_stays_silent() {
        let out = convert_all(2, 48_000, &[&vec![0.0; 9_600]]);
        assert!(!out.is_empty());
        assert!(out.iter().all(|&s| s == 0));
    }

    #[test]
    fn the_resampler_carries_its_place_across_callbacks() {
        // The same ramp fed whole or in uneven pieces must give the same samples.
        let ramp: Vec<f32> = (0..4_410).map(|i| i as f32 / 4_410.0).collect();
        let whole = convert_all(1, 44_100, &[&ramp]);
        let split = convert_all(1, 44_100, &[&ramp[..1], &ramp[1..1_000], &ramp[1_000..]]);
        assert_eq!(whole, split);
        assert!(whole.len() > 1_000);
    }

    #[test]
    fn interpolation_follows_a_ramp() {
        // 16 kHz in and out: every sample passes through unchanged (to the last one).
        let input = [0.0, 0.25, 0.5, 0.75, 1.0];
        let out = convert_all(1, 16_000, &[&input]);
        assert_eq!(out, [0, 8192, 16384, 24575]);
        // 32 kHz in: every second input sample, so the ramp is halved in length.
        let out = convert_all(1, 32_000, &[&[0.0, 0.1, 0.2, 0.3, 0.4]]);
        assert_eq!(out, [0, to_i16(0.2)]);
    }

    #[test]
    fn samples_are_clamped_to_full_scale() {
        assert_eq!(to_i16(2.0), i16::MAX);
        assert_eq!(to_i16(-2.0), -i16::MAX);
        assert_eq!(to_i16(0.0), 0);
    }

    #[test]
    fn downmix_averages_the_channels_and_drops_a_partial_frame() {
        assert_eq!(downmix(&[1.0, 0.0, 0.5, 0.5, 9.0], 2), [0.5, 0.5]);
        assert_eq!(downmix(&[0.1, 0.2, 0.3], 1), [0.1, 0.2, 0.3]);
        assert_eq!(downmix(&[0.3, 0.6, 0.9], 3).len(), 1);
    }

    #[test]
    fn write_chunks_writes_whole_100ms_chunks_and_keeps_the_rest() {
        let mut pending: Vec<i16> = (0..(WINDOW_SAMPLES + 5) as i16).collect();
        let mut out = Vec::new();
        write_chunks(&mut pending, &mut out).unwrap();
        assert_eq!(out.len(), WINDOW_SAMPLES * 2);
        assert_eq!(&out[..4], [0, 0, 1, 0]);
        assert_eq!(pending.len(), 5);
        write_chunks(&mut pending, &mut out).unwrap();
        assert_eq!(out.len(), WINDOW_SAMPLES * 2, "5 samples are not a chunk");
    }

    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn pump_stops_quietly_when_the_reader_closes_the_pipe() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Samples(vec![0; WINDOW_SAMPLES])).unwrap();
        pump(&rx, &mut ClosedPipe, false, Duration::from_millis(10)).unwrap();
    }

    #[test]
    fn pump_reports_a_stream_error() {
        let (tx, rx) = mpsc::channel();
        tx.send(Event::Failed("device unplugged".into())).unwrap();
        let err = pump(&rx, &mut Vec::new(), false, Duration::from_millis(10)).unwrap_err();
        assert!(format!("{err:#}").contains("device unplugged"), "{err:#}");
    }

    #[test]
    fn pump_fills_gaps_with_silence_only_when_asked() {
        // No audio at all: a closed pipe ends the run on the first silence chunk.
        let (tx, rx) = mpsc::channel::<Event>();
        pump(&rx, &mut ClosedPipe, true, Duration::from_millis(10)).unwrap();
        drop(tx);

        // Without fill, nothing is written, so the run ends only when the stream does.
        let (tx, rx) = mpsc::channel::<Event>();
        drop(tx);
        let mut out = Vec::new();
        let err = pump(&rx, &mut out, false, Duration::from_millis(10)).unwrap_err();
        assert!(out.is_empty());
        assert!(format!("{err:#}").contains("ended"), "{err:#}");
    }

    #[test]
    fn source_names_parse() {
        assert_eq!("mic".parse(), Ok(CaptureSource::Mic));
        assert_eq!("system".parse(), Ok(CaptureSource::System));
        assert!("speaker".parse::<CaptureSource>().is_err());
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_returns_a_clear_error() {
        let err = run_capture(CaptureSource::Mic, Vec::new()).unwrap_err();
        assert!(format!("{err:#}").contains("Windows and macOS"), "{err:#}");
    }
}
