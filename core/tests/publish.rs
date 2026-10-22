//! `publish` end to end with a fake claude CLI (a shell script).
#![cfg(unix)]

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use common::*;
use mn_core::publish::{parse_artifact_url, publish_with_config_dir};
use mn_core::summarize::PUBLISH_PENDING;
use mn_core::{Config, Status};
use serde_json::{json, Value};

const INDEX_HTML: &str = include_str!("../../artifact/index.html");
const URL: &str = "https://claude.example/artifact/existing-id";

struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Fixture {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn notes(&self) -> PathBuf {
        self.dir.path().join("notes")
    }

    fn config_dir(&self) -> PathBuf {
        self.dir.path().join("config")
    }

    fn url_file(&self) -> PathBuf {
        self.config_dir()
            .join("meeting-notes")
            .join("artifact_url.txt")
    }

    fn cfg(&self, claude: &FakeCli) -> Config {
        Config {
            notes_dir: self.notes(),
            claude_cli: claude.path.clone(),
            ..Config::default()
        }
    }

    fn fake(&self, exit: i32, stdout: &str) -> FakeCli {
        FakeCli::new(self.dir.path(), "claude", exit, stdout, None)
    }

    /// Three meetings: an old published one, and two pending ones (one newer than the other).
    fn three_sessions(&self) -> [PathBuf; 3] {
        let notes = self.notes();
        let old = make_session(
            &notes,
            "2026-03-02_0900",
            "2026-03-02T09:00:00+02:00",
            "2026-03-02T09:30:00+02:00",
        );
        add_meeting(
            &old,
            "Old one",
            "2026-03-02T09:00:00+02:00",
            "2026-03-02T09:35:00+02:00",
            false,
        );
        let mid = make_session(
            &notes,
            "2026-03-03_1400",
            "2026-03-03T14:00:00+02:00",
            "2026-03-03T14:30:00+02:00",
        );
        add_meeting(
            &mid,
            "Middle one",
            "2026-03-03T14:00:00+02:00",
            "2026-03-03T14:36:00+02:00",
            true,
        );
        let new = make_session(&notes, "2026-03-04_1000", STARTED, ENDED);
        add_meeting(&new, "New one", STARTED, "2026-03-04T10:50:00+02:00", true);
        [old, mid, new]
    }
}

fn run(cfg: &Config, config_dir: &Path) -> (anyhow::Result<()>, Vec<Value>) {
    let seen = RefCell::new(Vec::new());
    let result = publish_with_config_dir(cfg, config_dir, |s: Status| {
        seen.borrow_mut().push(serde_json::to_value(s).unwrap())
    });
    (result, seen.into_inner())
}

fn prompt_of(call: &Call) -> &str {
    assert_eq!(call.argv[0], "-p");
    assert_eq!(&call.argv[2..], ["--allowedTools", "Artifact"]);
    &call.argv[1]
}

/// The JSON object after "and files " in the prompt.
fn files_map(prompt: &str) -> Value {
    let start = prompt.find("and files ").unwrap() + "and files ".len();
    let mut values = serde_json::Deserializer::from_str(&prompt[start..]).into_iter::<Value>();
    values.next().unwrap().unwrap()
}

#[test]
fn staging_dir_has_the_page_the_index_and_only_the_pending_meetings() {
    let _guard = lock();
    let fx = Fixture::new();
    let [_old, _mid, _new] = fx.three_sessions();
    let claude = fx.fake(0, "OK data/index.json");
    let cfg = Config {
        artifact_url: Some(URL.to_string()),
        ..fx.cfg(&claude)
    };

    let (result, statuses) = run(&cfg, &fx.config_dir());
    result.unwrap();

    assert_eq!(claude.call_count(), 1);
    let call = claude.call(1);
    let stage = &call.cwd_copy;
    assert_eq!(
        fs::read_to_string(stage.join("index.html")).unwrap(),
        INDEX_HTML
    );

    let index = read_json(&stage.join("data/index.json"));
    chrono::DateTime::parse_from_rfc3339(index["updated_at"].as_str().unwrap()).unwrap();
    assert_eq!(
        index["meetings"],
        json!([
            {"id": "2026-03-04_1000", "title": "New one", "started_at": STARTED, "generated_at": "2026-03-04T10:50:00+02:00"},
            {"id": "2026-03-03_1400", "title": "Middle one", "started_at": "2026-03-03T14:00:00+02:00", "generated_at": "2026-03-03T14:36:00+02:00"},
            {"id": "2026-03-02_0900", "title": "Old one", "started_at": "2026-03-02T09:00:00+02:00", "generated_at": "2026-03-02T09:35:00+02:00"},
        ])
    );

    let mut staged: Vec<String> = fs::read_dir(stage.join("data/meetings"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    staged.sort();
    assert_eq!(staged, ["2026-03-03_1400.json", "2026-03-04_1000.json"]);
    for id in ["2026-03-03_1400", "2026-03-04_1000"] {
        assert_eq!(
            read_json(&stage.join(format!("data/meetings/{id}.json"))),
            read_json(&fx.notes().join(id).join("meeting.json"))
        );
        assert_eq!(
            fs::read_to_string(stage.join(format!("data/transcripts/{id}.md"))).unwrap(),
            fs::read_to_string(fx.notes().join(id).join("transcript.md")).unwrap()
        );
    }
    assert!(!stage.join("data/transcripts/2026-03-02_0900.md").exists());

    // the real run happened in a staging dir under notes_dir/.staging, now gone. The fake's
    // cwd may be the resolved path (macOS temp dirs: /var -> /private/var), so accept either.
    let staging = fx.notes().join(".staging");
    let staging_real = fs::canonicalize(fx.notes()).unwrap().join(".staging");
    assert!(
        call.cwd.starts_with(&staging) || call.cwd.starts_with(&staging_real),
        "{} is not under {}",
        call.cwd.display(),
        staging_real.display()
    );
    assert!(!call.cwd.exists());

    assert_eq!(call.artifact_env.as_deref(), Some("1"));
    assert_eq!(call.stdin, "");
    let prompt = prompt_of(&call);
    assert!(prompt.starts_with(&format!(
        "Update the existing artifact {URL}: first read it with action read, then list its files with action list, scope files and url set to it"
    )), "{prompt}");
    assert!(prompt.contains(
        "Then publish with url set to it, file_path ./index.html (unchanged, do not edit it, omit capabilities so the stored ones are kept), and files {"
    ));
    assert!(prompt.contains(
        "If the publish is still refused because some files were not read, read each path it names with action read and path"
    ));
    assert!(prompt
        .ends_with("Reply with only OK plus the list of published files, or the exact error."));
    let files = files_map(prompt);
    let expected: Value = json!({
        "data/index.json": "data/index.json",
        "data/meetings/2026-03-03_1400.json": "data/meetings/2026-03-03_1400.json",
        "data/transcripts/2026-03-03_1400.md": "data/transcripts/2026-03-03_1400.md",
        "data/meetings/2026-03-04_1000.json": "data/meetings/2026-03-04_1000.json",
        "data/transcripts/2026-03-04_1000.md": "data/transcripts/2026-03-04_1000.md",
    });
    assert_eq!(files, expected);

    assert_eq!(
        statuses,
        vec![
            json!({"state": "publishing", "meetings": 2}),
            json!({"state": "published", "url": URL}),
        ]
    );
}

#[test]
fn success_removes_the_markers_of_the_published_sessions() {
    let _guard = lock();
    let fx = Fixture::new();
    let [old, mid, new] = fx.three_sessions();
    let claude = fx.fake(0, "OK\ndata/index.json");
    let cfg = Config {
        artifact_url: Some(URL.to_string()),
        ..fx.cfg(&claude)
    };
    run(&cfg, &fx.config_dir()).0.unwrap();

    for dir in [&old, &mid, &new] {
        assert!(!dir.join(PUBLISH_PENDING).exists());
        assert!(dir.join("meeting.json").exists(), "meeting.json stays");
    }
    let staging = fx.notes().join(".staging");
    assert!(!staging.exists() || fs::read_dir(staging).unwrap().next().is_none());

    // nothing pending now: no second call
    let (result, statuses) = run(&cfg, &fx.config_dir());
    result.unwrap();
    assert_eq!(claude.call_count(), 1);
    assert!(statuses.is_empty());
}

#[test]
fn failure_keeps_the_markers_and_cleans_the_staging_dir() {
    let _guard = lock();
    for (exit, stdout) in [(1, "OK but exit 1"), (0, "Error: artifact was not read")] {
        let fx = Fixture::new();
        let [_old, mid, new] = fx.three_sessions();
        let claude = fx.fake(exit, stdout);
        let cfg = Config {
            artifact_url: Some(URL.to_string()),
            ..fx.cfg(&claude)
        };
        let (result, statuses) = run(&cfg, &fx.config_dir());
        let message = format!("{:#}", result.unwrap_err());
        assert!(message.contains(stdout), "{message}");

        assert!(mid.join(PUBLISH_PENDING).exists());
        assert!(new.join(PUBLISH_PENDING).exists());
        assert_eq!(
            statuses,
            vec![json!({"state": "publishing", "meetings": 2})]
        );
        let staging = fx.notes().join(".staging");
        assert!(!staging.exists() || fs::read_dir(staging).unwrap().next().is_none());
    }
}

#[test]
fn first_run_creates_the_artifact_and_saves_its_url() {
    let _guard = lock();
    let fx = Fixture::new();
    let [_old, _mid, new] = fx.three_sessions();
    let claude = fx.fake(0, "OK\nhttps://claude.ai/artifact/abc-123_XYZ\n");
    let cfg = fx.cfg(&claude);
    assert!(cfg.artifact_url.is_none());

    let (result, statuses) = run(&cfg, &fx.config_dir());
    result.unwrap();

    let prompt = claude.call(1).argv[1].clone();
    assert!(prompt.starts_with("Publish ./index.html as a new artifact using the Artifact tool, unchanged, with capabilities {\"db\":{},\"user\":{},\"downloads\":true,\"sample\":{}}, icon \"calendar\", title \"My Meetings\", and files {"), "{prompt}");
    assert!(
        prompt.ends_with(
            "Reply with only OK and the artifact URL on the next line, or the exact error."
        ),
        "{prompt}"
    );
    assert_eq!(files_map(&prompt).as_object().unwrap().len(), 5);

    assert_eq!(
        fs::read_to_string(fx.url_file()).unwrap().trim(),
        "https://claude.ai/artifact/abc-123_XYZ"
    );
    assert_eq!(
        statuses.last().unwrap(),
        &json!({"state": "published", "url": "https://claude.ai/artifact/abc-123_XYZ"})
    );
    assert!(!new.join(PUBLISH_PENDING).exists());

    // the next publish updates that artifact
    add_meeting(&new, "New one", STARTED, "2026-03-04T11:00:00+02:00", true);
    run(&cfg, &fx.config_dir()).0.unwrap();
    let second = claude.call(2).argv[1].clone();
    assert!(
        second.starts_with("Update the existing artifact https://claude.ai/artifact/abc-123_XYZ: "),
        "{second}"
    );
}

#[test]
fn first_run_without_a_url_in_the_reply_fails_and_keeps_markers() {
    let _guard = lock();
    let fx = Fixture::new();
    let [_old, _mid, new] = fx.three_sessions();
    let claude = fx.fake(0, "OK");
    let cfg = fx.cfg(&claude);

    let message = format!("{:#}", run(&cfg, &fx.config_dir()).0.unwrap_err());
    assert!(message.contains("URL"), "{message}");
    assert!(new.join(PUBLISH_PENDING).exists());
    assert!(!fx.url_file().exists());
}

#[test]
fn url_comes_from_config_first_then_the_url_file() {
    let _guard = lock();
    let fx = Fixture::new();
    fx.three_sessions();
    let claude = fx.fake(0, "OK");
    fs::create_dir_all(fx.url_file().parent().unwrap()).unwrap();
    fs::write(
        fx.url_file(),
        "https://claude.example/artifact/from-file\nsecond line\n",
    )
    .unwrap();

    let cfg = fx.cfg(&claude);
    run(&cfg, &fx.config_dir()).0.unwrap();
    assert!(claude.call(1).argv[1]
        .starts_with("Update the existing artifact https://claude.example/artifact/from-file: "));

    let sessions = fx.notes().join("2026-03-04_1000");
    add_meeting(
        &sessions,
        "New one",
        STARTED,
        "2026-03-04T11:00:00+02:00",
        true,
    );
    let cfg = Config {
        artifact_url: Some("https://claude.example/artifact/from-config".to_string()),
        ..cfg
    };
    run(&cfg, &fx.config_dir()).0.unwrap();
    assert!(claude.call(2).argv[1]
        .starts_with("Update the existing artifact https://claude.example/artifact/from-config: "));
}

#[test]
fn claude_config_dir_is_exported_only_when_configured() {
    let _guard = lock();
    let fx = Fixture::new();
    let [_old, _mid, new] = fx.three_sessions();
    let claude = fx.fake(0, "OK");
    let cfg = Config {
        artifact_url: Some(URL.to_string()),
        ..fx.cfg(&claude)
    };
    run(&cfg, &fx.config_dir()).0.unwrap();
    assert_eq!(
        claude.call(1).config_dir,
        std::env::var("CLAUDE_CONFIG_DIR").ok()
    );

    add_meeting(&new, "New one", STARTED, "2026-03-04T11:00:00+02:00", true);
    let cfg = Config {
        claude_config_dir: Some(PathBuf::from("/example/claude-company")),
        ..cfg
    };
    run(&cfg, &fx.config_dir()).0.unwrap();
    assert_eq!(
        claude.call(2).config_dir.as_deref(),
        Some("/example/claude-company")
    );
}

#[test]
fn init_page_creates_the_artifact_with_no_meetings() {
    let _guard = lock();
    let fx = Fixture::new();
    let claude = fx.fake(0, "OK\nhttps://claude.ai/artifact/new-page_1\n");
    let cfg = fx.cfg(&claude);
    let seen = RefCell::new(Vec::new());
    mn_core::publish::init_page_with_config_dir(&cfg, &fx.config_dir(), |s: Status| {
        seen.borrow_mut().push(serde_json::to_value(s).unwrap())
    })
    .unwrap();

    let prompt = claude.call(1).argv[1].clone();
    assert!(
        prompt.starts_with("Publish ./index.html as a new artifact"),
        "{prompt}"
    );
    let files = files_map(&prompt);
    assert_eq!(files.as_object().unwrap().len(), 1, "{files}");
    assert!(files.get("data/index.json").is_some());
    assert_eq!(
        fs::read_to_string(fx.url_file()).unwrap().trim(),
        "https://claude.ai/artifact/new-page_1"
    );
    assert_eq!(
        seen.into_inner().last().unwrap(),
        &json!({"state": "published", "url": "https://claude.ai/artifact/new-page_1"})
    );
}

#[test]
fn nothing_pending_does_not_call_claude() {
    let _guard = lock();
    let fx = Fixture::new();
    let claude = fx.fake(0, "OK");
    let cfg = fx.cfg(&claude);
    let (result, statuses) = run(&cfg, &fx.config_dir());
    result.unwrap(); // notes dir does not even exist
    assert_eq!(claude.call_count(), 0);
    assert!(statuses.is_empty());
}

#[test]
fn a_pending_session_without_a_transcript_is_an_error_with_context() {
    let _guard = lock();
    let fx = Fixture::new();
    let [_old, mid, _new] = fx.three_sessions();
    fs::remove_file(mid.join("transcript.md")).unwrap();
    let claude = fx.fake(0, "OK");
    let cfg = Config {
        artifact_url: Some(URL.to_string()),
        ..fx.cfg(&claude)
    };
    let message = format!("{:#}", run(&cfg, &fx.config_dir()).0.unwrap_err());
    assert!(message.contains("transcript.md"), "{message}");
    assert_eq!(claude.call_count(), 0);
}

#[test]
fn artifact_url_is_found_in_the_reply() {
    assert_eq!(
        parse_artifact_url("OK\nhttps://claude.ai/artifact/abc-123\n").as_deref(),
        Some("https://claude.ai/artifact/abc-123")
    );
    assert_eq!(
        parse_artifact_url("OK (https://claude.ai/artifact/abc-123).").as_deref(),
        Some("https://claude.ai/artifact/abc-123")
    );
    assert_eq!(parse_artifact_url("OK https://example.com/x"), None);
    assert_eq!(parse_artifact_url("OK"), None);
}
