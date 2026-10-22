//! Shared helpers for the summarize / publish / process tests: fake CLIs (shell scripts)
//! that record how they were called and print canned output.
#![allow(dead_code)]
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use mn_core::session::{SessionInfo, StopReason, TrackSeconds};
use serde_json::{json, Value};

// Writing an executable while another test forks can give ETXTBSY; run these one at a time.
static LOCK: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// What one call to a fake CLI looked like.
pub struct Call {
    pub argv: Vec<String>,
    pub stdin: String,
    pub cwd: PathBuf,
    /// `None` when CLAUDE_CONFIG_DIR was not set.
    pub config_dir: Option<String>,
    pub artifact_env: Option<String>,
    /// A copy of the files in the working directory at call time.
    pub cwd_copy: PathBuf,
}

pub struct FakeCli {
    pub path: PathBuf,
    rec: PathBuf,
}

impl FakeCli {
    /// Prints `stdout`, exits with `exit`. If the call has `-o <file>`, copies `out_file` there.
    pub fn new(dir: &Path, name: &str, exit: i32, stdout: &str, out_file: Option<&str>) -> Self {
        let path = dir.join(name);
        let rec = dir.join(format!("{name}-rec"));
        fs::create_dir_all(&rec).unwrap();
        fs::write(rec.join("stdout"), stdout).unwrap();
        if let Some(out) = out_file {
            fs::write(rec.join("out_canned"), out).unwrap();
        }
        let script = format!(
            r#"#!/bin/sh
rec='{rec}'
echo x >> "$rec/count"
n=$(wc -l < "$rec/count" | tr -d ' ')
printf '%s\0' "$@" > "$rec/argv.$n"
cat > "$rec/stdin.$n"
pwd > "$rec/cwd.$n"
printf '%s' "${{CLAUDE_CONFIG_DIR-__unset__}}" > "$rec/config_dir.$n"
printf '%s' "${{CLAUDE_CODE_ARTIFACT-__unset__}}" > "$rec/artifact_env.$n"
mkdir -p "$rec/cwd_copy.$n"
cp -r . "$rec/cwd_copy.$n/" 2>/dev/null
f=stdout
if [ -f "$rec/stdout_alt" ]; then
  case " $* " in *" --json-schema "*) ;; *) f=stdout_alt ;; esac
fi
while [ $# -gt 0 ]; do
  [ "$1" = -o ] && out=$2
  [ "$1" = --output-schema ] && cp "$2" "$rec/schema.$n"
  shift
done
[ -n "$out" ] && [ -f "$rec/out_canned" ] && cp "$rec/out_canned" "$out"
cat "$rec/$f"
exit {exit}
"#,
            rec = rec.display()
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        FakeCli { path, rec }
    }

    /// Calls without `--json-schema` print this instead (the publish call of a combined fake).
    pub fn with_alt_stdout(self, stdout: &str) -> Self {
        fs::write(self.rec.join("stdout_alt"), stdout).unwrap();
        self
    }

    pub fn call_count(&self) -> usize {
        fs::read_to_string(self.rec.join("count"))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }

    /// 1-based.
    pub fn call(&self, n: usize) -> Call {
        let read = |name: &str| fs::read_to_string(self.rec.join(format!("{name}.{n}"))).unwrap();
        let opt = |name: &str| Some(read(name)).filter(|v| v != "__unset__");
        let mut argv: Vec<String> = read("argv").split('\0').map(String::from).collect();
        argv.pop(); // the empty text after the last NUL
        Call {
            argv,
            stdin: read("stdin"),
            cwd: PathBuf::from(read("cwd").trim_end()),
            config_dir: opt("config_dir"),
            artifact_env: opt("artifact_env"),
            cwd_copy: self.rec.join(format!("cwd_copy.{n}")),
        }
    }

    pub fn schema_copy(&self, n: usize) -> String {
        fs::read_to_string(self.rec.join(format!("schema.{n}"))).unwrap()
    }
}

pub const STARTED: &str = "2026-03-04T10:00:00+02:00";
pub const ENDED: &str = "2026-03-04T10:47:40+02:00";

/// A made-up card.
pub fn sample_card() -> Value {
    json!({
        "title": "Weekly sync with Example Co",
        "title_source": "calendar",
        "project": "Project Example",
        "client": null,
        "attendees": ["Sam Example", "Alex Sample"],
        "related_links": [],
        "summary": ["Agreed the plan."],
        "decisions": ["Ship on Friday"],
        "deliverables": [],
        "my_todos": [],
        "waiting_on": []
    })
}

pub fn claude_ok_output(card: &Value) -> String {
    json!({
        "type": "result",
        "subtype": "success",
        "is_error": false,
        "result": "",
        "structured_output": card,
        "num_turns": 3,
        "total_cost_usd": 0.12
    })
    .to_string()
}

/// `<notes>/<name>` with session.json and a transcript.md.
pub fn make_session(notes: &Path, name: &str, started: &str, ended: &str) -> PathBuf {
    let dir = notes.join(name);
    fs::create_dir_all(&dir).unwrap();
    SessionInfo {
        started_at: started.to_string(),
        ended_at: ended.to_string(),
        stop_reason: StopReason::Silence,
        seconds: TrackSeconds {
            me: 10.0,
            them: 20.0,
        },
        audio_deleted: true,
    }
    .save(&dir)
    .unwrap();
    fs::write(
        dir.join("transcript.md"),
        format!("# Transcript {name}\n\n[00:00:01] **Me:** Hello from {name}.\n"),
    )
    .unwrap();
    dir
}

/// Adds meeting.json (and the pending marker) the way summarize would.
pub fn add_meeting(dir: &Path, title: &str, started: &str, generated_at: &str, pending: bool) {
    let id = dir.file_name().unwrap().to_string_lossy().into_owned();
    let mut meeting = sample_card();
    meeting["title"] = json!(title);
    meeting["id"] = json!(id);
    meeting["started_at"] = json!(started);
    meeting["generated_at"] = json!(generated_at);
    fs::write(
        dir.join("meeting.json"),
        serde_json::to_string_pretty(&meeting).unwrap(),
    )
    .unwrap();
    if pending {
        fs::write(dir.join(mn_core::summarize::PUBLISH_PENDING), "").unwrap();
    }
}

pub fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}
