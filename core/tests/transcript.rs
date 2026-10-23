use mn_core::capture::Track;
use mn_core::transcribe::*;

const FIXTURE: &str = include_str!("fixtures/whisper-sample.json");

fn seg(start_ms: u64, end_ms: u64, speaker: Track, text: &str) -> Segment {
    Segment {
        start_ms,
        end_ms,
        speaker,
        text: text.to_string(),
    }
}

#[test]
fn parses_the_real_whisper_fixture() {
    let segments = parse_whisper_json(FIXTURE, Track::Them).unwrap();
    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].start_ms, 0);
    assert_eq!(segments[0].end_ms, 11_000);
    assert_eq!(segments[0].speaker, Track::Them);
    assert_eq!(
        segments[0].text,
        "And so my fellow Americans, ask not what your country can do for you, ask what you can do for your country."
    );
}

#[test]
fn rejects_json_without_transcription() {
    assert!(parse_whisper_json("{\"result\":{}}", Track::Me).is_err());
}

#[test]
fn drops_known_silence_hallucinations() {
    for text in [
        "you",
        "You",
        " Thank you. ",
        "THANKS FOR WATCHING!",
        "[BLANK_AUDIO]",
        "[Music]",
        "(silence)",
        "",
        "   ",
    ] {
        assert!(is_hallucination(text), "{text:?} should be dropped");
    }
    for text in ["Thank you for the update", "you know", "Hello"] {
        assert!(!is_hallucination(text), "{text:?} should be kept");
    }
}

#[test]
fn drops_wrong_language_and_token_loop_garbage() {
    // from a noisy 60 s recording with `-l auto`
    assert!(is_garbage("වවවවවවවවවව"));
    assert!(is_garbage(&"වවවවවවවවවව".repeat(8)));
    assert!(is_garbage(&"ἀἜἀἀἀἀἀ".repeat(6)));
    // one character 8+ times in a row, even inside otherwise normal words
    assert!(is_garbage("Okay........ so"));
    assert!(is_garbage("Hmmmmmmmm"));
    // long text with almost no variety
    assert!(is_garbage(&"ab".repeat(15)));
}

#[test]
fn keeps_normal_text_in_any_script() {
    assert!(!is_garbage("Thanks, I will send the schedule today."));
    assert!(!is_garbage("سنرسل الجدول الزمني اليوم"));
    assert!(!is_garbage("Hello"));
    // 7 in a row is fine, and 20 chars is not "longer than 20"
    assert!(!is_garbage("Hmmmmmmm"));
    assert!(!is_garbage(&"ab".repeat(10)));
}

#[test]
fn keeps_a_long_normal_sentence_even_though_letters_repeat() {
    let text = "We reviewed the quarterly budget and agreed that the marketing team should \
        receive an additional allocation, provided that the finance department signs off \
        before the end of the month. Meanwhile, engineering will finish the migration, \
        document the new deployment process, and schedule a short demo for the whole \
        company next week.";
    let distinct = text.chars().collect::<std::collections::HashSet<_>>().len();
    // a plain "distinct < 15% of length" rule would wrongly drop this
    assert!(distinct * 100 < 15 * text.chars().count());
    assert!(!is_garbage(text));
    assert!(!is_garbage(
        "سنرسل الجدول الزمني اليوم ثم نراجع الميزانية مع فريق التسويق ونحدد موعد الاجتماع القادم مع العميل بعد أن ينتهي قسم المالية من مراجعة الأرقام النهائية للربع الثاني من هذا العام"
    ));
}

#[test]
fn merge_sorts_by_start_and_joins_same_speaker_within_2s() {
    let merged = merge(vec![
        seg(12_000, 13_000, Track::Me, "Next"),
        seg(7_000, 8_000, Track::Me, "with the budget"),
        seg(1_500, 3_000, Track::Them, "Hello"),
        seg(0, 1_000, Track::Me, "Hi everyone"),
        seg(5_000, 6_000, Track::Me, "Let's start"),
    ]);
    assert_eq!(
        merged,
        vec![
            seg(0, 1_000, Track::Me, "Hi everyone"),
            seg(1_500, 3_000, Track::Them, "Hello"),
            seg(5_000, 8_000, Track::Me, "Let's start with the budget"),
            seg(12_000, 13_000, Track::Me, "Next"),
        ]
    );
}

#[test]
fn merge_does_not_join_across_the_other_speaker() {
    let merged = merge(vec![
        seg(0, 1_000, Track::Me, "One"),
        seg(1_200, 1_800, Track::Them, "Two"),
        seg(2_000, 3_000, Track::Me, "Three"),
    ]);
    assert_eq!(merged.len(), 3);
}

#[test]
fn formats_header_and_speaker_lines() {
    let md = format_transcript(
        "# Transcript 2026-10-22 10:49–11:32",
        &[
            seg(0, 1_000, Track::Me, "Hi everyone"),
            seg(3_723_000, 3_725_000, Track::Them, "Bye"),
        ],
    );
    assert_eq!(
        md,
        "# Transcript 2026-10-22 10:49–11:32\n\n[00:00:00] **Me:** Hi everyone\n[01:02:03] **Them:** Bye\n"
    );
}

#[test]
fn a_long_line_repeated_on_silence_is_kept_twice() {
    let loop_line = "اشتركوا في القناة يا جماعة";
    let segments: Vec<Segment> = (0..20)
        .map(|i| seg(i * 1000, i * 1000 + 900, Track::Me, loop_line))
        .chain([seg(30_000, 31_000, Track::Me, "We ship on Thursday then.")])
        .collect();
    let kept = drop_repeats(segments);
    assert_eq!(kept.len(), 3);
    assert_eq!(kept[2].text, "We ship on Thursday then.");
}

#[test]
fn repeats_ignore_case_and_punctuation_but_short_replies_stay() {
    let segments = vec![
        seg(0, 1, Track::Them, "Thank you very much."),
        seg(2, 3, Track::Them, "thank you very much"),
        seg(4, 5, Track::Them, "Thank you, very much!"),
        seg(6, 7, Track::Them, "Okay."),
        seg(8, 9, Track::Them, "Okay."),
        seg(10, 11, Track::Them, "Okay."),
    ];
    let kept: Vec<String> = drop_repeats(segments).into_iter().map(|s| s.text).collect();
    assert_eq!(
        kept,
        [
            "Thank you very much.",
            "thank you very much",
            "Okay.",
            "Okay.",
            "Okay."
        ]
    );
}

#[test]
fn a_segment_spanning_a_vad_gap_starts_just_before_its_end() {
    // Seen with VAD: a short line given the end of the previous speech stretch as its start.
    let fixed = fix_long_span(seg(66_540, 160_460, Track::Me, "تمام بالنسبة للديدلين"));
    assert_eq!(fixed.end_ms, 160_460);
    assert_eq!(fixed.start_ms, 160_460 - 21 * 80);

    let normal = seg(1_000, 9_000, Track::Me, "A normal eight second line.");
    assert_eq!(fix_long_span(normal.clone()), normal);
}
