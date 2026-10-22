use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;

use crate::config::Config;

/// How much of a CLI's output goes into an error message.
const REASON_CHARS: usize = 300;

/// A `claude` command with CLAUDE_CONFIG_DIR exported only when the config sets it.
pub(crate) fn claude_command(cfg: &Config) -> Command {
    let mut cmd = Command::new(&cfg.claude_cli);
    if let Some(dir) = &cfg.claude_config_dir {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    cmd
}

/// Runs the command to the end with stdout and stderr captured.
/// `stdin` is written from a thread so a big prompt cannot deadlock against a big reply.
pub(crate) fn run(cmd: &mut Command, stdin: Option<&str>) -> io::Result<Output> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let Some(input) = stdin else {
        return cmd.stdin(Stdio::null()).output();
    };
    let mut child = cmd.stdin(Stdio::piped()).spawn()?;
    let mut pipe = child.stdin.take().expect("stdin is piped");
    let input = input.to_owned();
    let writer = thread::spawn(move || {
        // A CLI that exits early closes the pipe; its exit status tells the story.
        let _ = pipe.write_all(input.as_bytes());
    });
    let out = child.wait_with_output();
    let _ = writer.join();
    out
}

/// The first 300 characters of stdout, or of stderr when stdout is empty.
pub(crate) fn reason(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let text = if stdout.trim().is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        stdout.into_owned()
    };
    snippet(&text)
}

pub(crate) fn snippet(text: &str) -> String {
    text.trim().chars().take(REASON_CHARS).collect()
}

/// True when `cli` is a path to a file, or a bare name found on PATH.
pub(crate) fn resolves(cli: &Path) -> bool {
    if cli.components().count() > 1 || cli.is_absolute() {
        return cli.is_file();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(cli).is_file()))
        .unwrap_or(false)
}
