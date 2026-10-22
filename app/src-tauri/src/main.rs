#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::io;

use mn_core::CaptureSource;

/// `<this exe> capture mic|system`: the core's capture command, so the default capture
/// commands work from the widget, which has no `mn` next to it. Returns the exit code.
fn capture(source: Option<&str>) -> i32 {
    let run = || -> Result<(), String> {
        let source = source
            .ok_or("usage: capture mic|system")?
            .parse::<CaptureSource>()?;
        mn_core::run_capture(source, io::stdout().lock()).map_err(|err| format!("{err:#}"))
    };
    match run() {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("error: {message}");
            1
        }
    }
}

fn main() {
    // Runs before any Tauri or GTK setup (and before GDK_BACKEND below): the capture child
    // must be a plain process with no window.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("capture") {
        std::process::exit(capture(args.get(2).map(String::as_str)));
    }

    // Wayland does not let an app place its own window or grab global keys. Running under
    // XWayland does, so ask GTK for X11 before Tauri/GTK starts. A value the user set wins.
    #[cfg(target_os = "linux")]
    if std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }
    meeting_notes_app::run();
}
