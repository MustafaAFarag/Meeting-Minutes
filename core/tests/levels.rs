use mn_core::levels::*;

fn sine(amplitude: f64, samples: usize) -> Vec<i16> {
    (0..samples)
        .map(|i| {
            let t = i as f64 / SAMPLE_RATE as f64;
            (amplitude * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as i16
        })
        .collect()
}

fn write_wav(path: &std::path::Path, samples: impl IntoIterator<Item = i16>) {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        writer.write_sample(s).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn silence_is_very_low() {
    assert!(rms_db(&[0; 1600]) <= -90.0);
    assert!(rms_db(&[]) <= -90.0);
}

#[test]
fn full_scale_sine_is_about_minus_3_db() {
    let db = rms_db(&sine(32767.0, 16_000));
    assert!((db + 3.01).abs() < 0.1, "got {db}");
}

#[test]
fn half_scale_sine_is_about_minus_9_db() {
    let db = rms_db(&sine(16384.0, 16_000));
    assert!((db + 9.03).abs() < 0.1, "got {db}");
}

#[test]
fn wav_peak_is_the_loudest_100ms_window() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wav");
    write_wav(&path, sine(32767.0, 16_000));
    let db = wav_peak_db(&path).unwrap();
    assert!((db + 3.01).abs() < 0.1, "got {db}");
}

#[test]
fn wav_peak_sees_a_short_quiet_burst_that_whole_file_rms_would_miss() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wav");
    // 0.2 s of a +-400 square wave (-38.3 dBFS), then 60 s of digital silence:
    // whole-file RMS is about -63 dB, but two windows are well above -50.
    let burst = (0..3200).map(|i| if i % 2 == 0 { 400 } else { -400 });
    write_wav(&path, burst.chain(std::iter::repeat_n(0, 960_000)));
    let db = wav_peak_db(&path).unwrap();
    assert!((db + 38.3).abs() < 0.2, "got {db}");
}

#[test]
fn wav_peak_of_digital_silence_is_very_low() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.wav");
    write_wav(&path, std::iter::repeat_n(0, 32_000));
    assert!(wav_peak_db(&path).unwrap() <= -90.0);
}

#[test]
fn meter_reports_each_complete_window_and_drains() {
    let mut meter = WindowMeter::new();
    let loud = sine(32767.0, WINDOW_SAMPLES);
    // quiet window, loud window, then half a window that must wait for more samples
    meter.push(&[0; WINDOW_SAMPLES]);
    meter.push(&loud);
    meter.push(&loud[..WINDOW_SAMPLES / 2]);
    let windows = meter.take_windows();
    assert_eq!(windows.len(), 2);
    assert!(windows[0] <= -90.0);
    assert!((windows[1] + 3.01).abs() < 0.2);
    assert!(meter.take_windows().is_empty(), "windows drain after take");
    meter.push(&loud[WINDOW_SAMPLES / 2..]);
    let windows = meter.take_windows();
    assert_eq!(windows.len(), 1, "carried samples complete a window");
    assert!((windows[0] + 3.01).abs() < 0.2);
}

#[test]
fn meter_with_no_complete_window_reports_nothing() {
    let mut meter = WindowMeter::new();
    meter.push(&sine(32767.0, WINDOW_SAMPLES - 1));
    assert!(meter.take_windows().is_empty());
}
