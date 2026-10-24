use anyhow::anyhow;
use mn_core::failure::{classify, ErrorKind, Failure, Stage};
use mn_core::Status;
use serde::Deserialize;

#[test]
fn classify_maps_representative_errors_to_plain_messages() {
    let cases = [
        (
            "no_sound",
            "No sound was recorded.",
            "Check that Meeting Notes may use the microphone (and on a Mac, Screen & System Audio Recording), then try a short test.",
            "both audio tracks were silent",
            Stage::Transcribe,
        ),
        (
            "mic_unavailable",
            "The microphone could not be opened.",
            "Check your microphone is connected and allowed for Meeting Notes.",
            "no default input device (microphone)",
            Stage::Record,
        ),
        (
            "whisper",
            "The speech-to-text step failed.",
            "Try again. If it fails again, use Report a problem.",
            "whisper exited with code 1",
            Stage::Transcribe,
        ),
        (
            "claude_signed_out",
            "Claude is signed out, so the notes could not be written.",
            "Open Claude and sign in, then click Try again.",
            "claude is not logged in",
            Stage::Summarize,
        ),
        (
            "claude_failed",
            "Claude could not write the notes.",
            "Try again in a few minutes. If it keeps failing, use Report a problem.",
            "claude returned no structured_output",
            Stage::Summarize,
        ),
        (
            "network",
            "No internet connection.",
            "Connect to the internet, then click Try again.",
            "network is unreachable",
            Stage::Publish,
        ),
        (
            "publish",
            "Your notes were written but could not be added to My Meetings.",
            "Click Try again. Your notes are saved on this computer.",
            "publishing failed",
            Stage::Publish,
        ),
        (
            "config",
            "Meeting Notes is not set up correctly.",
            "Ask Claude to repair the install, or use Report a problem.",
            "invalid config value",
            Stage::Other,
        ),
        (
            "unknown",
            "Something went wrong.",
            "Click Try again. If it happens again, use Report a problem.",
            "unclassified failure",
            Stage::Other,
        ),
    ];

    for (expected, expected_message, expected_hint, detail, stage) in cases {
        let (kind, message, hint) = classify(&anyhow!(detail), stage);
        assert_eq!(serde_json::to_value(kind).unwrap(), expected, "{detail}");
        assert_eq!(message, expected_message, "{detail}");
        assert_eq!(hint, expected_hint, "{detail}");
    }
}

#[test]
fn fake_sensitive_error_is_scrubbed_and_never_reaches_the_plain_message() {
    let detail = format!(
        "person@example.test https://client.test/private {} {}",
        dirs::home_dir().unwrap_or_default().display(),
        "meeting words ".repeat(80)
    );
    let scrubbed = mn_core::logging::scrub(&detail);
    let err = anyhow!("{detail}");
    let (_, message, hint) = classify(&err, Stage::Summarize);
    let home_path = dirs::home_dir().unwrap_or_default().display().to_string();

    for secret in [
        "person@example.test",
        "https://client.test",
        home_path.as_str(),
    ] {
        assert!(!scrubbed.contains(secret), "{scrubbed}");
        assert!(!message.contains(secret), "{message}");
        assert!(!hint.contains(secret), "{hint}");
    }
    assert!(!message.contains("meeting words"), "{message}");
    assert!(!hint.contains("meeting words"), "{hint}");
    assert!(scrubbed.chars().count() <= 500);
}

#[test]
fn typed_failure_controls_classification_and_preserves_stage() {
    let failure = Failure {
        kind: ErrorKind::ClaudeSignedOut,
        stage: Stage::Summarize,
        detail: "auth status said signed out".to_string(),
    };
    let err = anyhow::Error::new(failure);

    let (kind, message, hint) = classify(&err, Stage::Other);

    assert_eq!(kind, ErrorKind::ClaudeSignedOut);
    assert_eq!(
        message,
        "Claude is signed out, so the notes could not be written."
    );
    assert_eq!(hint, "Open Claude and sign in, then click Try again.");
}

#[derive(Deserialize)]
struct OldConsumer {
    state: String,
    message: String,
}

#[test]
fn status_error_adds_contract_fields_and_old_consumers_still_parse() {
    let value = serde_json::to_value(Status::Error {
        message: "Something went wrong.".to_string(),
        kind: ErrorKind::Unknown,
        stage: Stage::Other,
        hint: "Click Try again. If it happens again, use Report a problem.".to_string(),
    })
    .unwrap();

    assert_eq!(value["state"], "error");
    assert_eq!(value["message"], "Something went wrong.");
    assert_eq!(value["kind"], "unknown");
    assert_eq!(value["stage"], "other");
    assert!(value["hint"].as_str().is_some());
    let old: OldConsumer = serde_json::from_value(value).unwrap();
    assert_eq!(old.state, "error");
    assert_eq!(old.message, "Something went wrong.");

    let legacy: Status = serde_json::from_str(r#"{"state":"error","message":"legacy"}"#).unwrap();
    match legacy {
        Status::Error {
            kind, stage, hint, ..
        } => {
            assert_eq!(kind, ErrorKind::Unknown);
            assert_eq!(stage, Stage::Other);
            assert_eq!(hint, ErrorKind::Unknown.hint());
        }
        other => panic!("expected legacy Error status, got {other:?}"),
    }
}
