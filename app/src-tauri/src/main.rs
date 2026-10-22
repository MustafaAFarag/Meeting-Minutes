#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Wayland does not let an app place its own window or grab global keys. Running under
    // XWayland does, so ask GTK for X11 before Tauri/GTK starts. A value the user set wins.
    #[cfg(target_os = "linux")]
    if std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }
    meeting_notes_app::run();
}
