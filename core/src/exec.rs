use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;

use crate::config::Config;

/// How much of a CLI's output goes into an error message.
const REASON_CHARS: usize = 300;

/// Windows `CREATE_NO_WINDOW`: a console program started by a GUI app gets no console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Every child process the core starts goes through here. On Windows it hides the console
/// window and finds a bare name that an npm install ships as `name.cmd`.
/// A `.cmd` is spawned by its full path: std runs it through `cmd.exe` and escapes the
/// arguments for it, which a hand-written `cmd /C` would not.
pub(crate) fn command(program: impl AsRef<Path>) -> Command {
    let program = program.as_ref();
    let resolved = if cfg!(windows) && is_bare(program) {
        find_on_path(program)
    } else {
        None
    };
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut cmd = Command::new(resolved.as_deref().unwrap_or(program));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The file names to try for a bare program name on Windows: `.exe`, then `.cmd`.
/// A name that already has one of the runnable extensions is tried as it is.
fn name_candidates(name: &str) -> Vec<String> {
    let lower = name.to_ascii_lowercase();
    if [".exe", ".cmd", ".bat", ".com"]
        .iter()
        .any(|ext| lower.ends_with(ext))
    {
        return vec![name.to_string()];
    }
    vec![format!("{name}.exe"), format!("{name}.cmd")]
}

fn is_bare(program: &Path) -> bool {
    program.components().count() <= 1 && !program.is_absolute()
}

/// The first file on PATH that matches the bare name (with the Windows extensions on Windows).
fn find_on_path(name: &Path) -> Option<PathBuf> {
    let candidates = if cfg!(windows) {
        name_candidates(name.to_str()?)
    } else {
        vec![name.to_str()?.to_string()]
    };
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths).find_map(|dir| {
        candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|path| path.is_file())
    })
}

/// A `claude` command with CLAUDE_CONFIG_DIR exported only when the config sets it.
pub(crate) fn claude_command(cfg: &Config) -> Command {
    let mut cmd = command(&cfg.claude_cli);
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
    if is_bare(cli) {
        find_on_path(cli).is_some()
    } else {
        cli.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_names_try_exe_then_cmd() {
        assert_eq!(name_candidates("claude"), ["claude.exe", "claude.cmd"]);
        assert_eq!(name_candidates("codex"), ["codex.exe", "codex.cmd"]);
    }

    #[test]
    fn names_with_a_runnable_extension_are_kept() {
        assert_eq!(name_candidates("whisper-cli.exe"), ["whisper-cli.exe"]);
        assert_eq!(name_candidates("Claude.CMD"), ["Claude.CMD"]);
        assert_eq!(name_candidates("tool.bat"), ["tool.bat"]);
    }

    #[test]
    fn other_dots_do_not_count_as_an_extension() {
        assert_eq!(name_candidates("tool.v2"), ["tool.v2.exe", "tool.v2.cmd"]);
    }

    #[test]
    fn bare_means_a_single_relative_component() {
        assert!(is_bare(Path::new("claude")));
        assert!(!is_bare(Path::new("bin/claude")));
        assert!(!is_bare(Path::new("./claude")));
        assert!(!is_bare(Path::new("/usr/bin/claude")));
    }
}
