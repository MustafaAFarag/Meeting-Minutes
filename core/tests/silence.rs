//! The auto-stop state machine, driven with synthetic window levels (no real time).
//!
//! "Me" is a room mic: it has a noise floor, so a window counts as sound only if it stands out from
//! that floor. "Them" is digital system audio that really drops to about -100 dB between speech, so
//! it only uses the absolute threshold.
use mn_core::levels::SilenceTracker;

const SPEECH: f64 = -20.0;
const QUIET: f64 = -80.0;
const DIGITAL: f64 = -100.0;

/// Room noise of a real mic, all above the fixed -50 dB threshold.
fn room_noise(i: usize) -> f64 {
    [-48.0, -47.0, -46.0, -45.0][i % 4]
}

/// threshold -50, margin 10, floor clamp -40: the defaults
fn tracker(silence_seconds: u64) -> SilenceTracker {
    SilenceTracker::new(-50.0, 10.0, -40.0, silence_seconds)
}

/// One 100 ms window per track.
fn tick(t: &mut SilenceTracker, me: f64, them: f64) {
    t.push(&[me], &[them]);
}

fn ticks(t: &mut SilenceTracker, n: usize, me: f64, them: f64) {
    for _ in 0..n {
        tick(t, me, them);
    }
}

#[test]
fn silence_timer_does_not_start_before_first_sound() {
    let mut t = tracker(2);
    ticks(&mut t, 10_000, QUIET, QUIET);
    assert!(!t.should_stop());
    assert_eq!(t.silent_for(), 0);
}

#[test]
fn stops_after_n_seconds_of_both_quiet() {
    let mut t = tracker(2);
    ticks(&mut t, 5, QUIET, QUIET);
    tick(&mut t, SPEECH, QUIET);
    ticks(&mut t, 19, QUIET, QUIET);
    assert!(!t.should_stop());
    assert_eq!(t.silent_for(), 1);
    tick(&mut t, QUIET, QUIET);
    assert!(t.should_stop());
    assert_eq!(t.silent_for(), 2);
}

#[test]
fn either_track_making_sound_resets_the_timer() {
    let mut t = tracker(2);
    ticks(&mut t, 5, QUIET, QUIET);
    tick(&mut t, SPEECH, SPEECH);
    ticks(&mut t, 19, QUIET, QUIET);
    tick(&mut t, QUIET, SPEECH); // only "them" speaks
    assert_eq!(t.silent_for(), 0);
    ticks(&mut t, 19, QUIET, QUIET);
    assert!(!t.should_stop());
    tick(&mut t, SPEECH, QUIET); // only "me" speaks
    ticks(&mut t, 19, QUIET, QUIET);
    assert!(!t.should_stop());
    tick(&mut t, QUIET, QUIET);
    assert!(t.should_stop());
}

#[test]
fn a_window_exactly_at_the_threshold_is_sound() {
    // "me": floor -80, so the absolute threshold -50 decides
    let mut t = tracker(1);
    ticks(&mut t, 5, QUIET, QUIET);
    tick(&mut t, -50.0, QUIET);
    ticks(&mut t, 9, QUIET, QUIET);
    assert!(!t.should_stop());
    tick(&mut t, QUIET, QUIET);
    assert!(t.should_stop(), "-50 dB started the timer");

    let mut t = tracker(1);
    ticks(&mut t, 5, QUIET, QUIET);
    tick(&mut t, -50.1, QUIET);
    ticks(&mut t, 1000, QUIET, QUIET);
    assert!(!t.should_stop(), "-50.1 dB never started the timer");

    // "them"
    let mut t = tracker(1);
    tick(&mut t, QUIET, -50.0);
    ticks(&mut t, 10, QUIET, QUIET);
    assert!(t.should_stop());

    let mut t = tracker(1);
    tick(&mut t, QUIET, -50.1);
    ticks(&mut t, 1000, QUIET, QUIET);
    assert!(!t.should_stop());
}

// --- Them: absolute threshold only ---------------------------------------------------------

#[test]
fn continuous_system_audio_at_minus_42_db_never_counts_as_silent() {
    let mut t = tracker(2);
    for _ in 0..300 {
        // 30 s of speech from the very first window
        tick(&mut t, QUIET, -42.0);
        assert_eq!(t.silent_for(), 0);
        assert!(!t.should_stop());
    }
    // once it really goes quiet, the normal timer runs
    ticks(&mut t, 19, QUIET, DIGITAL);
    assert!(!t.should_stop());
    tick(&mut t, QUIET, DIGITAL);
    assert!(t.should_stop());
}

#[test]
fn system_audio_has_no_noise_floor() {
    // -45 dB is sound for "them" however long it lasts, unlike the same level on the mic
    let mut t = tracker(1);
    ticks(&mut t, 700, QUIET, -45.0);
    assert_eq!(t.silent_for(), 0);
    ticks(&mut t, 10, QUIET, DIGITAL);
    assert!(t.should_stop());
}

// --- Me: adaptive floor, clamped ------------------------------------------------------------

#[test]
fn mic_with_room_noise_and_normal_speech_stops_after_n_seconds_of_noise_only() {
    let mut t = tracker(2);
    // 30 s of room noise at -48..-45 dB: above the fixed threshold, but never sound
    for i in 0..300 {
        tick(&mut t, room_noise(i), DIGITAL);
    }
    assert!(!t.should_stop());
    assert_eq!(t.silent_for(), 0, "timer must not start on noise alone");

    // speech from -35 to -20 dB in 0.5 s bursts, 1.5 s of noise between them (under the 2 s limit)
    for burst in [-35.0, -30.0, -20.0] {
        ticks(&mut t, 5, burst, DIGITAL);
        for i in 0..15 {
            tick(&mut t, room_noise(i), DIGITAL);
        }
        assert!(!t.should_stop());
    }
    // the quietest normal speech, then 2 s of noise only
    tick(&mut t, -35.0, DIGITAL);
    for i in 0..19 {
        tick(&mut t, room_noise(i), DIGITAL);
    }
    assert!(!t.should_stop());
    assert_eq!(t.silent_for(), 1);
    tick(&mut t, room_noise(19), DIGITAL);
    assert!(t.should_stop());
}

#[test]
fn constant_minus_48_floor_with_speech_bursts_stops_after_n_seconds() {
    let mut t = tracker(2);
    ticks(&mut t, 100, -48.0, DIGITAL);
    assert!(!t.should_stop());
    assert_eq!(t.silent_for(), 0);
    tick(&mut t, -35.0, DIGITAL); // floor -48 + 10 = -38
    ticks(&mut t, 10, -48.0, DIGITAL);
    tick(&mut t, -20.0, DIGITAL);
    ticks(&mut t, 19, -48.0, DIGITAL);
    assert!(!t.should_stop());
    tick(&mut t, -48.0, DIGITAL);
    assert!(t.should_stop());
}

#[test]
fn sound_must_clear_the_noise_floor_by_the_margin() {
    // floor -45, margin 10: the bar is -35
    let mut t = tracker(1);
    ticks(&mut t, 20, -45.0, DIGITAL);
    tick(&mut t, -36.0, DIGITAL);
    ticks(&mut t, 1000, -45.0, DIGITAL);
    assert!(!t.should_stop(), "-36 dB is under floor + margin");

    for sound in [-35.0, -34.0] {
        let mut t = tracker(1);
        ticks(&mut t, 20, -45.0, DIGITAL);
        tick(&mut t, sound, DIGITAL);
        ticks(&mut t, 9, -45.0, DIGITAL);
        assert!(!t.should_stop());
        tick(&mut t, -45.0, DIGITAL);
        assert!(t.should_stop(), "{sound} dB is sound");
    }
}

#[test]
fn continuous_mic_speech_from_the_start_never_counts_as_silent() {
    // No quiet history at all: the raw 10th percentile would be the speech level itself.
    // The floor is clamped at -40, so -28 dB is above -40 + 10.
    let mut t = tracker(2);
    for _ in 0..300 {
        tick(&mut t, -28.0, DIGITAL);
        assert_eq!(t.silent_for(), 0);
        assert!(!t.should_stop());
    }
    // when the speech ends, the room noise is quiet even though the history was all speech
    for i in 0..19 {
        tick(&mut t, room_noise(i), DIGITAL);
    }
    assert!(!t.should_stop());
    tick(&mut t, room_noise(19), DIGITAL);
    assert!(t.should_stop());
}

#[test]
fn noise_floor_is_the_10th_percentile_so_speech_does_not_raise_it() {
    // 80% of windows are speech at -33 dB (just above the -35 bar of a -45 floor); a median or
    // mean floor would sit at -33, and the clamp would put the bar at -30, hearing nothing
    let mut t = tracker(1);
    for _ in 0..30 {
        ticks(&mut t, 8, -33.0, DIGITAL);
        ticks(&mut t, 2, -45.0, DIGITAL);
    }
    ticks(&mut t, 7, -45.0, DIGITAL);
    assert!(!t.should_stop());
    tick(&mut t, -45.0, DIGITAL);
    assert!(t.should_stop());
}

#[test]
fn noise_floor_only_remembers_the_last_60_seconds() {
    let mut t = tracker(1);
    // 60 s in a very quiet room, then the room gets noisier (a fan starts)
    ticks(&mut t, 600, -70.0, DIGITAL);
    ticks(&mut t, 700, -45.0, DIGITAL);
    // the old floor has aged out, so the new noise is quiet again and the timer ran out
    assert!(t.should_stop());
}

#[test]
fn digital_silence_track_counts_as_silent() {
    let mut t = tracker(1);
    ticks(&mut t, 10, DIGITAL, DIGITAL);
    tick(&mut t, SPEECH, DIGITAL);
    ticks(&mut t, 9, DIGITAL, DIGITAL);
    assert!(!t.should_stop());
    tick(&mut t, DIGITAL, DIGITAL);
    assert!(t.should_stop());
}

// --- ticks --------------------------------------------------------------------------------

#[test]
fn a_track_with_no_windows_this_tick_is_not_sound() {
    let mut t = tracker(1);
    ticks(&mut t, 5, QUIET, QUIET);
    tick(&mut t, SPEECH, QUIET);
    for _ in 0..10 {
        t.push(&[], &[]);
    }
    assert!(t.should_stop(), "a stalled stream counts as quiet");
}

#[test]
fn several_windows_in_one_tick_all_feed_the_floor() {
    let mut t = tracker(1);
    t.push(&[QUIET, QUIET, QUIET], &[QUIET]);
    t.push(&[SPEECH, QUIET], &[]);
    for _ in 0..9 {
        t.push(&[QUIET], &[QUIET]);
    }
    assert!(!t.should_stop());
    t.push(&[QUIET], &[QUIET]);
    assert!(t.should_stop());
}
