//! Quoting a Linux autostart `Exec=` line.
//!
//! `auto-launch` 0.6 writes `~/.config/autostart/{app name}.desktop` and puts the
//! executable on `Exec=` with no quotes. The plugin's app name is the product name
//! (`package_info().name`, "Meeting Notes"), so the file is `Meeting Notes.desktop`.
//! The path is fixed up after the plugin writes that file. Windows and macOS are untouched.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `~/.config/autostart/{app_name}.desktop`, matching auto-launch's XDG path
/// (home + `.config` + `autostart`, not `XDG_CONFIG_HOME`).
pub fn autostart_desktop_path(home: &Path, app_name: &str) -> PathBuf {
    home.join(".config")
        .join("autostart")
        .join(format!("{app_name}.desktop"))
}

/// The program login should start: `$APPIMAGE` when it is set and non-empty, otherwise
/// the current executable. An AppImage's `current_exe` is the temporary mount.
pub fn launch_target(appimage: Option<&str>, current_exe: &str) -> String {
    match appimage {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => current_exe.to_string(),
    }
}

/// Wraps `path` in double quotes for a desktop entry `Exec=` value.
/// `"`, `` ` ``, `$` and `\` are escaped with a backslash.
pub fn quote_desktop_exec(path: &str) -> String {
    let mut quoted = String::with_capacity(path.len() + 2);
    quoted.push('"');
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// Replaces the `Exec=` line with the quoted `launch_path`. Other lines stay as they are.
pub fn rewrite_desktop_exec(contents: &str, launch_path: &str) -> String {
    let quoted = quote_desktop_exec(launch_path);
    let mut out = String::with_capacity(contents.len() + quoted.len());
    let mut replaced = false;
    for line in contents.split_inclusive('\n') {
        let (body, ending) = match line.strip_suffix('\n') {
            Some(rest) if rest.ends_with('\r') => (&rest[..rest.len() - 1], "\r\n"),
            Some(rest) => (rest, "\n"),
            None => (line, ""),
        };
        let trimmed = body.trim_start();
        if !replaced && trimmed.starts_with("Exec=") {
            let indent_len = body.len() - trimmed.len();
            out.push_str(&body[..indent_len]);
            out.push_str("Exec=");
            out.push_str(&quoted);
            out.push_str(ending);
            replaced = true;
        } else {
            out.push_str(line);
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str("Exec=");
        out.push_str(&quoted);
        out.push('\n');
    }
    out
}

/// Rewrites `Exec=` in an existing autostart file. Does not create a file.
pub fn rewrite_autostart_file(path: &Path, launch_path: &str) -> Result<(), String> {
    let contents = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(format!("no autostart file at {}", path.display()));
        }
        Err(err) => return Err(format!("could not read {}: {err}", path.display())),
    };
    let updated = rewrite_desktop_exec(&contents, launch_path);
    if updated == contents {
        return Ok(());
    }
    fs::write(path, updated).map_err(|err| format!("could not write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_file_uses_the_product_name() {
        let home = Path::new("jane");
        let path = autostart_desktop_path(home, "Meeting Notes");
        let parts: Vec<_> = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            parts,
            ["jane", ".config", "autostart", "Meeting Notes.desktop"]
        );
    }

    #[test]
    fn appimage_wins_over_the_mounted_exe() {
        assert_eq!(
            launch_target(
                Some("/home/jane/Meeting Notes.AppImage"),
                "/tmp/.mount_abc/meeting-notes-app"
            ),
            "/home/jane/Meeting Notes.AppImage"
        );
    }

    #[test]
    fn current_exe_is_used_when_appimage_is_unset_or_empty() {
        assert_eq!(
            launch_target(None, "/usr/bin/meeting-notes-app"),
            "/usr/bin/meeting-notes-app"
        );
        assert_eq!(
            launch_target(Some(""), "/usr/bin/meeting-notes-app"),
            "/usr/bin/meeting-notes-app"
        );
    }

    #[test]
    fn quotes_a_path_with_spaces() {
        let path = "/mnt/ssd/Project Management/meeting-notes-app";
        assert_eq!(
            quote_desktop_exec(path),
            "\"/mnt/ssd/Project Management/meeting-notes-app\""
        );
    }

    #[test]
    fn escapes_a_dollar_in_the_path() {
        assert_eq!(quote_desktop_exec("/tmp/foo$bar"), "\"/tmp/foo\\$bar\"");
    }

    #[test]
    fn escapes_quotes_backticks_and_backslashes() {
        assert_eq!(
            quote_desktop_exec("/tmp/a\"b`c\\d"),
            "\"/tmp/a\\\"b\\`c\\\\d\""
        );
    }

    #[test]
    fn rewrites_only_the_exec_line() {
        let path = "/mnt/ssd/Project Management/meeting-notes-app";
        let file = "\
[Desktop Entry]\n\
Type=Application\n\
Name=Meeting Notes\n\
Exec=/mnt/ssd/Project Management/meeting-notes-app \n\
StartupNotify=false\n\
Terminal=false";
        let updated = rewrite_desktop_exec(file, path);
        let exec = updated
            .lines()
            .find(|line| line.starts_with("Exec="))
            .unwrap();
        println!("rewritten Exec line: {exec}");
        assert_eq!(
            exec,
            "Exec=\"/mnt/ssd/Project Management/meeting-notes-app\""
        );
        assert!(updated.contains("Name=Meeting Notes\n"));
        assert!(updated.contains("Terminal=false"));
        assert!(!updated.contains("Exec=/mnt/ssd"));
    }

    #[test]
    fn rewrite_writes_the_quoted_exec_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meeting Notes.desktop");
        fs::write(
            &path,
            "Exec=/tmp/Project Files/meeting-notes-app \nTerminal=false\n",
        )
        .unwrap();
        rewrite_autostart_file(&path, "/tmp/Project Files/meeting-notes-app").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        let exec = text.lines().find(|line| line.starts_with("Exec=")).unwrap();
        println!("rewritten Exec line: {exec}");
        assert_eq!(exec, "Exec=\"/tmp/Project Files/meeting-notes-app\"");
        assert!(text.contains("Terminal=false\n"));
    }

    #[test]
    fn rewrite_refuses_to_create_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.desktop");
        let err = rewrite_autostart_file(&path, "/tmp/app").unwrap_err();
        assert!(!path.exists(), "{err}");
    }
}
