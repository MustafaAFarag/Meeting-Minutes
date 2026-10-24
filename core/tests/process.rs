//! `process` runs only the stages that are not done yet.
#![cfg(unix)]

mod common;

use std::cell::RefCell;
use std::fs;

use common::*;
use mn_core::summarize::PUBLISH_PENDING;
use mn_core::{process, Config, Status};
use serde_json::Value;

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        common::use_test_log_dir();
        Fixture {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    /// One fake claude that answers a summarize call with a card and a publish call with OK.
    fn claude(&self) -> FakeCli {
        FakeCli::new(
            self.dir.path(),
            "claude",
            0,
            &claude_ok_output(&sample_card()),
            None,
        )
        .with_alt_stdout("OK data/index.json")
    }

    fn cfg(&self, claude: &FakeCli) -> Config {
        Config {
            notes_dir: self.dir.path().join("notes"),
            claude_cli: claude.path.clone(),
            artifact_url: Some("https://claude.example/artifact/existing-id".to_string()),
            ..Config::default()
        }
    }
}

fn run(cfg: &Config, session: &std::path::Path) -> (anyhow::Result<()>, Vec<Value>) {
    let seen = RefCell::new(Vec::new());
    let result = process(cfg, session, |s: Status| {
        seen.borrow_mut().push(serde_json::to_value(s).unwrap())
    });
    (result, seen.into_inner())
}

fn states(statuses: &[Value]) -> Vec<&str> {
    statuses
        .iter()
        .map(|s| s["state"].as_str().unwrap())
        .collect()
}

#[test]
fn summarizes_and_publishes_when_only_the_transcript_exists() {
    let _guard = lock();
    let fx = Fixture::new();
    let claude = fx.claude();
    let cfg = fx.cfg(&claude);
    let session = make_session(&cfg.notes_dir, "2026-03-04_1000", STARTED, ENDED);

    let (result, statuses) = run(&cfg, &session);
    result.unwrap();

    assert_eq!(claude.call_count(), 2);
    assert!(claude.call(1).argv.contains(&"--json-schema".to_string()));
    assert_eq!(claude.call(2).argv.last().unwrap(), "Artifact");
    assert_eq!(
        states(&statuses),
        ["summarizing", "summarized", "publishing", "published"]
    );
    assert!(session.join("card.json").exists());
    assert!(!session.join(PUBLISH_PENDING).exists());
}

#[test]
fn skips_summarize_when_the_card_exists_and_only_publishes() {
    let _guard = lock();
    let fx = Fixture::new();
    let claude = fx.claude();
    let cfg = fx.cfg(&claude);
    let session = make_session(&cfg.notes_dir, "2026-03-04_1000", STARTED, ENDED);
    fs::write(session.join("card.json"), sample_card().to_string()).unwrap();
    add_meeting(
        &session,
        "Weekly sync",
        STARTED,
        "2026-03-04T10:50:00+02:00",
        true,
    );

    let (result, statuses) = run(&cfg, &session);
    result.unwrap();

    assert_eq!(claude.call_count(), 1, "only the publish call");
    assert_eq!(claude.call(1).argv.last().unwrap(), "Artifact");
    assert_eq!(states(&statuses), ["publishing", "published"]);
}

#[test]
fn everything_done_means_no_calls() {
    let _guard = lock();
    let fx = Fixture::new();
    let claude = fx.claude();
    let cfg = fx.cfg(&claude);
    let session = make_session(&cfg.notes_dir, "2026-03-04_1000", STARTED, ENDED);
    fs::write(session.join("card.json"), sample_card().to_string()).unwrap();
    add_meeting(
        &session,
        "Weekly sync",
        STARTED,
        "2026-03-04T10:50:00+02:00",
        false,
    );

    let (result, statuses) = run(&cfg, &session);
    result.unwrap();
    assert_eq!(claude.call_count(), 0);
    assert_eq!(statuses, Vec::<Value>::new());
}

#[test]
fn transcribes_first_when_the_transcript_is_missing() {
    let _guard = lock();
    let fx = Fixture::new();
    let claude = fx.claude();
    let cfg = fx.cfg(&claude);
    let session = make_session(&cfg.notes_dir, "2026-03-04_1000", STARTED, ENDED);
    fs::remove_file(session.join("transcript.md")).unwrap();

    // no audio in the session, so the transcribe stage is where it stops
    let (result, statuses) = run(&cfg, &session);
    let message = format!("{:#}", result.unwrap_err());
    assert!(
        message.contains("whisper_model") || message.contains("me.wav"),
        "{message}"
    );
    assert_eq!(claude.call_count(), 0);
    assert_eq!(statuses, Vec::<Value>::new());
}
