//! `summarize` end to end with fake claude / codex CLIs (shell scripts).
#![cfg(unix)]

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};

use common::*;
use mn_core::session::SessionInfo;
use mn_core::summarize::{
    build_prompt, fill_template, m365_step, summarize, Engine, PUBLISH_PENDING,
};
use mn_core::{Config, Status};
use serde_json::{json, Value};

const M365_DOC: &str = include_str!("../../docs/m365-step.md");
const SCHEMA: &str = include_str!("../../prompts/card.schema.json");

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

    fn session(&self) -> PathBuf {
        make_session(&self.notes(), "2026-03-04_1000", STARTED, ENDED)
    }

    fn cfg(&self, claude: &FakeCli, codex: Option<&FakeCli>) -> Config {
        Config {
            notes_dir: self.notes(),
            claude_cli: claude.path.clone(),
            codex_cli: codex
                .map_or_else(|| self.dir.path().join("no-such-codex"), |c| c.path.clone()),
            my_name: Some("Sam Example".to_string()),
            ..Config::default()
        }
    }

    fn fake(&self, name: &str, exit: i32, stdout: &str, out: Option<&str>) -> FakeCli {
        FakeCli::new(self.dir.path(), name, exit, stdout, out)
    }
}

fn statuses_of(
    f: impl FnOnce(&dyn Fn(Status)) -> anyhow::Result<PathBuf>,
) -> (anyhow::Result<PathBuf>, Vec<Value>) {
    let seen = RefCell::new(Vec::new());
    let result = f(&|s| seen.borrow_mut().push(serde_json::to_value(s).unwrap()));
    (result, seen.into_inner())
}

fn section(name: &str) -> String {
    let marker = format!("## {name}\n");
    let start = M365_DOC.find(&marker).unwrap() + marker.len();
    let rest = &M365_DOC[start..];
    let end = rest.find("\n## ").unwrap_or(rest.len());
    rest[..end].trim().to_string()
}

#[test]
fn fill_template_replaces_known_placeholders_once() {
    let out = fill_template(
        "a {{X}} b {{Y}} {{X}} {{UNKNOWN}} {{open",
        &[("X", "{{Y}}"), ("Y", "why")],
    );
    assert_eq!(out, "a {{Y}} b why {{Y}} {{UNKNOWN}} {{open");
}

#[test]
fn m365_sections_are_copied_verbatim_from_the_doc() {
    let claude = m365_step(Engine::Claude).unwrap();
    let codex = m365_step(Engine::Codex).unwrap();
    assert_eq!(claude, section("claude"));
    assert_eq!(codex, section("codex"));
    assert!(claude.contains("outlook_calendar_search"));
    assert!(codex.contains("No calendar access"));
}

#[test]
fn prompt_has_every_placeholder_filled() {
    let fx = Fixture::new();
    let session = fx.session();
    let info = SessionInfo::load(&session).unwrap().unwrap();
    let claude_fake = fx.fake("claude", 0, "", None);
    let cfg = fx.cfg(&claude_fake, None);

    let claude = build_prompt(&cfg, &session, &info, Engine::Claude).unwrap();
    assert!(!claude.contains("{{"), "{claude}");
    assert!(claude.contains(&format!("Session folder: {}", session.display())));
    assert!(claude.contains(&format!("started {STARTED} and ended {ENDED}")));
    assert!(claude.contains("Today is 2026-03-04 (Wednesday)."));
    assert!(claude.contains("PM's name if known: Sam Example"));
    assert!(claude.contains(&section("claude")));
    assert!(!claude.contains(&section("codex")));

    let codex = build_prompt(&cfg, &session, &info, Engine::Codex).unwrap();
    assert!(codex.contains(&section("codex")));
    assert!(!codex.contains(&section("claude")));

    let anon = Config {
        my_name: None,
        ..cfg
    };
    let prompt = build_prompt(&anon, &session, &info, Engine::Claude).unwrap();
    assert!(prompt.contains("PM's name if known: unknown"));
}

#[test]
fn claude_success_writes_card_meeting_and_marker() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let card = sample_card();
    let claude = fx.fake("claude", 0, &claude_ok_output(&card), None);
    let cfg = fx.cfg(&claude, None);

    let (result, statuses) = statuses_of(|on| summarize(&cfg, &session, on));
    let card_path = result.unwrap();

    assert_eq!(
        card_path,
        fs::canonicalize(&session).unwrap().join("card.json")
    );
    assert_eq!(read_json(&card_path), card, "card.json is the card exactly");

    let meeting = read_json(&session.join("meeting.json"));
    let mut expected = card.clone();
    let obj = expected.as_object_mut().unwrap();
    obj.insert("id".into(), json!("2026-03-04_1000"));
    obj.insert("started_at".into(), json!(STARTED));
    obj.insert("ended_at".into(), json!(ENDED));
    obj.insert("duration_min".into(), json!(48)); // 47 min 40 s
    obj.insert("generated_by".into(), json!("claude"));
    let generated_at = meeting["generated_at"].as_str().unwrap().to_string();
    chrono::DateTime::parse_from_rfc3339(&generated_at).unwrap();
    obj.insert("generated_at".into(), json!(generated_at));
    assert_eq!(meeting, expected);
    assert!(session.join(PUBLISH_PENDING).exists());

    assert_eq!(
        statuses,
        vec![
            json!({"state": "summarizing", "engine": "claude"}),
            json!({"state": "summarized", "engine": "claude", "card": card_path}),
        ]
    );
}

#[test]
fn claude_is_called_with_the_documented_arguments() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let claude = fx.fake("claude", 0, &claude_ok_output(&sample_card()), None);
    let cfg = fx.cfg(&claude, None);
    summarize(&cfg, &session, |_| {}).unwrap();

    assert_eq!(claude.call_count(), 1);
    let call = claude.call(1);
    let a = &call.argv;
    assert_eq!(&a[..4], ["-p", "--output-format", "json", "--json-schema"]);
    let schema: Value = serde_json::from_str(&a[4]).unwrap();
    assert_eq!(schema, serde_json::from_str::<Value>(SCHEMA).unwrap());
    assert_eq!(a[5], "--allowedTools");
    assert_eq!(
        a[6],
        "Read,mcp__claude_ai_Microsoft_365__outlook_calendar_search,mcp__claude_ai_Microsoft_365__chat_message_search,mcp__claude_ai_Microsoft_365__sharepoint_search"
    );
    assert_eq!(
        &a[7..],
        ["--model", "claude-opus-5-5", "--effort", "medium"]
    );
    assert_eq!(a.len(), 11, "the prompt goes on stdin");
    assert_eq!(call.cwd, fs::canonicalize(&session).unwrap());
    assert!(call
        .stdin
        .contains(&format!("Session folder: {}", call.cwd.display())));
    assert!(call.stdin.contains("outlook_calendar_search"));
}

#[test]
fn claude_config_dir_is_exported_only_when_configured() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let claude = fx.fake("claude", 0, &claude_ok_output(&sample_card()), None);

    let cfg = fx.cfg(&claude, None);
    summarize(&cfg, &session, |_| {}).unwrap();
    assert_eq!(
        claude.call(1).config_dir,
        std::env::var("CLAUDE_CONFIG_DIR").ok(),
        "unset in the config: the inherited value is left alone"
    );

    let cfg = Config {
        claude_config_dir: Some(PathBuf::from("/example/claude-company")),
        ..cfg
    };
    summarize(&cfg, &session, |_| {}).unwrap();
    assert_eq!(
        claude.call(2).config_dir.as_deref(),
        Some("/example/claude-company")
    );
}

fn codex_args(call: &Call, session: &Path) {
    let a = &call.argv;
    assert_eq!(&a[..3], ["exec", "--skip-git-repo-check", "-C"]);
    assert_eq!(Path::new(&a[3]), fs::canonicalize(session).unwrap());
    assert_eq!(a[4], "--output-schema");
    assert_eq!(a[6], "-o");
    assert_eq!(a[8], "-", "the prompt goes on stdin");
    assert_eq!(a.len(), 9);
}

#[test]
fn claude_failures_fall_back_to_codex() {
    let _guard = lock();
    let cases: [(&str, i32, String, &str); 3] = [
        ("exit 1", 1, String::from("rate limit reached"), "exited with"),
        (
            "is_error",
            0,
            json!({"type":"result","subtype":"success","is_error":true,"result":"You've hit your limit"})
                .to_string(),
            "hit your limit",
        ),
        (
            "no structured_output",
            0,
            json!({"type":"result","subtype":"success","is_error":false,"result":"text only"})
                .to_string(),
            "structured_output",
        ),
    ];
    for (label, exit, stdout, reason_part) in cases {
        let fx = Fixture::new();
        let session = fx.session();
        let claude = fx.fake("claude", exit, &stdout, None);
        let mut codex_card = sample_card();
        codex_card["title"] = json!("From codex");
        let codex = fx.fake("codex", 0, "progress noise", Some(&codex_card.to_string()));
        let cfg = fx.cfg(&claude, Some(&codex));

        let (result, statuses) = statuses_of(|on| summarize(&cfg, &session, on));
        let card_path = result.unwrap_or_else(|e| panic!("{label}: {e:#}"));

        assert_eq!(read_json(&card_path), codex_card, "{label}");
        let meeting = read_json(&session.join("meeting.json"));
        assert_eq!(meeting["generated_by"], "codex", "{label}");
        assert_eq!(meeting["title"], "From codex");
        assert!(session.join(PUBLISH_PENDING).exists());

        assert_eq!(statuses.len(), 3, "{label}: {statuses:?}");
        assert_eq!(
            statuses[0],
            json!({"state": "summarizing", "engine": "claude"})
        );
        assert_eq!(statuses[1]["engine"], "codex");
        assert!(
            statuses[1]["reason"]
                .as_str()
                .unwrap()
                .contains(reason_part),
            "{label}: {statuses:?}"
        );
        assert_eq!(statuses[2]["state"], "summarized");
        assert_eq!(statuses[2]["engine"], "codex");

        let call = codex.call(1);
        codex_args(&call, &session);
        assert!(call.stdin.contains(&section("codex")), "{label}");
        assert!(!call.stdin.contains("outlook_calendar_search"), "{label}");
        assert_eq!(
            serde_json::from_str::<Value>(&codex.schema_copy(1)).unwrap(),
            serde_json::from_str::<Value>(SCHEMA).unwrap()
        );
        // the temp schema and out file are gone
        let names: Vec<_> = fs::read_dir(&session)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| !n.starts_with('.')),
            "{label}: leftover {names:?}"
        );
    }
}

#[test]
fn both_failing_errors_with_both_reasons_and_writes_nothing() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let claude = fx.fake("claude", 1, "claude says: out of quota", None);
    let codex = fx.fake("codex", 2, "codex says: not logged in", None);
    let cfg = fx.cfg(&claude, Some(&codex));

    let err = summarize(&cfg, &session, |_| {}).unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("out of quota"), "{message}");
    assert!(message.contains("not logged in"), "{message}");
    for name in ["card.json", "meeting.json", PUBLISH_PENDING] {
        assert!(!session.join(name).exists(), "{name} must not exist");
    }
    assert!(!session.join(".card.schema.json").exists());
}

#[test]
fn codex_output_that_is_not_an_object_fails() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let claude = fx.fake("claude", 1, "nope", None);
    let codex = fx.fake("codex", 0, "", Some("[1, 2]"));
    let cfg = fx.cfg(&claude, Some(&codex));

    let message = format!("{:#}", summarize(&cfg, &session, |_| {}).unwrap_err());
    assert!(message.contains("nope"), "{message}");
    assert!(message.contains("codex"), "{message}");
    assert!(!session.join("card.json").exists());
}

#[test]
fn missing_codex_is_reported_next_to_the_claude_reason() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let claude = fx.fake("claude", 1, "claude broke", None);
    let cfg = fx.cfg(&claude, None);

    let message = format!("{:#}", summarize(&cfg, &session, |_| {}).unwrap_err());
    assert!(message.contains("claude broke"), "{message}");
    assert!(message.contains("codex"), "{message}");
    assert!(message.contains("no-such-codex"), "{message}");
}

#[test]
fn reasons_are_cut_to_300_chars() {
    let _guard = lock();
    let fx = Fixture::new();
    let session = fx.session();
    let long = "x".repeat(1000);
    let claude = fx.fake("claude", 1, &long, None);
    let cfg = fx.cfg(&claude, None);

    let message = format!("{:#}", summarize(&cfg, &session, |_| {}).unwrap_err());
    assert!(message.contains(&"x".repeat(300)));
    assert!(!message.contains(&"x".repeat(301)));
}

#[test]
fn missing_transcript_is_an_error_with_context() {
    let fx = Fixture::new();
    let session = fx.session();
    fs::remove_file(session.join("transcript.md")).unwrap();
    let claude = fx.fake("claude", 0, "", None);
    let cfg = fx.cfg(&claude, None);

    let message = format!("{:#}", summarize(&cfg, &session, |_| {}).unwrap_err());
    assert!(message.contains("transcript.md"), "{message}");
}
