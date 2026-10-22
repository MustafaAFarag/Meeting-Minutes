mod meetings;
mod runner;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use mn_core::record::RecordOptions;
use mn_core::{Config, Status};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};
use tauri_plugin_opener::OpenerExt;

use runner::RunState;

/// The event the UI listens to. The payload is the core's `Status` as JSON.
const STATUS_EVENT: &str = "mn-status";
const WINDOW_LABEL: &str = "main";

type Shared<'a> = State<'a, Arc<RunState>>;

enum Job {
    Record,
    Process(PathBuf),
}

fn emit_status(app: &AppHandle, status: Status) {
    if let Err(e) = app.emit(STATUS_EVENT, &status) {
        eprintln!("could not send status to the window: {e}");
    }
}

/// `{:#}` on an anyhow error prints the whole cause chain on one line.
fn describe(err: impl std::fmt::Display) -> String {
    format!("{err:#}")
}

fn run_job(app: &AppHandle, state: &RunState, job: Job) -> Result<(), String> {
    let cfg = Config::load(None).map_err(describe)?;
    let emit = |status: Status| emit_status(app, status);
    let session = match job {
        Job::Record => {
            state.set_last_session(None);
            state.set_recording(true);
            let opts = RecordOptions {
                max_seconds: None,
                interrupt: Arc::clone(&state.interrupt),
            };
            let recorded = mn_core::record(&cfg, &opts, emit);
            state.set_recording(false);
            let dir = recorded.map_err(describe)?;
            state.set_last_session(Some(dir.clone()));
            dir
        }
        Job::Process(dir) => dir,
    };
    mn_core::process(&cfg, &session, emit).map_err(describe)
}

/// Starts `job` on a background thread. Errors only if a run is already active.
fn launch(app: &AppHandle, state: &Arc<RunState>, job: Job) -> Result<(), String> {
    let guard = state.begin()?;
    let app = app.clone();
    let state = Arc::clone(state);
    thread::Builder::new()
        .name("mn-run".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| run_job(&app, &state, job)))
                .unwrap_or_else(|_| Err("The app hit an unexpected problem.".to_string()));
            state.set_recording(false);
            // Free the slot before the final status, so "Try again" never races the guard.
            drop(guard);
            if let Err(message) = result {
                eprintln!("error: {message}");
                emit_status(&app, Status::Error { message });
            }
        })
        .map(|_| ())
        .map_err(|e| format!("Could not start: {e}"))
}

#[tauri::command]
fn start(app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    launch(&app, &state, Job::Record)
}

#[tauri::command]
fn stop(state: Shared<'_>) -> Result<(), String> {
    state.request_stop()
}

/// Runs the pipeline again on the last session. If the recording itself failed there is no
/// session yet, so this starts a fresh recording instead.
#[tauri::command]
fn retry(app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    let job = match state.last_session() {
        Some(dir) => Job::Process(dir),
        None => Job::Record,
    };
    launch(&app, &state, job)
}

#[tauri::command]
fn open_meetings(app: AppHandle) -> Result<(), String> {
    let cfg = Config::load(None).map_err(describe)?;
    let url = meetings::meetings_url(cfg.artifact_url.as_deref(), dirs::config_dir().as_deref())
        .inspect_err(|e| eprintln!("open_meetings: {e}"))?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| format!("Could not open the browser: {e}"))
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Meeting Notes")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "quit" => {
                let app = app.clone();
                let state = Arc::clone(&*app.state::<Arc<RunState>>());
                // Close the WAV files first, off the UI thread.
                thread::spawn(move || {
                    state.stop_recording_and_wait(Duration::from_secs(3));
                    app.exit(0);
                });
            }
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Arc::new(RunState::default()))
        .invoke_handler(tauri::generate_handler![start, stop, retry, open_meetings])
        .setup(|app| {
            build_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing hides to the tray; "Quit" in the tray menu is the only way out.
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running the Meeting Notes app");
}
