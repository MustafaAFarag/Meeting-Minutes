mod meetings;
mod notch;
mod runner;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use mn_core::record::RecordOptions;
use mn_core::{Config, Status};
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, Window, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_opener::OpenerExt;

use notch::{Rect, WidgetPrefs};
use runner::RunState;

/// The event the UI listens to. The payload is the core's `Status` as JSON.
const STATUS_EVENT: &str = "mn-status";
const WINDOW_LABEL: &str = "main";
/// After the window stops moving for this long, a drag counts as finished.
const DRAG_SETTLE: Duration = Duration::from_millis(250);

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

/// Where the pill hangs: the horizontal centre it keeps while resizing, and the top edge of the
/// monitor it is on (physical pixels).
#[derive(Clone, Copy)]
struct Anchor {
    center_x: i32,
    top_y: i32,
}

struct NotchState {
    prefs: Mutex<WidgetPrefs>,
    prefs_path: Option<PathBuf>,
    anchor: Mutex<Option<Anchor>>,
    /// Counts window moves, so only the last one of a drag is acted on.
    moves: AtomicU64,
}

impl NotchState {
    fn load() -> Self {
        let prefs_path = dirs::config_dir().map(|dir| notch::prefs_path(&dir));
        let prefs = prefs_path
            .as_deref()
            .map(WidgetPrefs::load)
            .unwrap_or_default();
        Self {
            prefs: Mutex::new(prefs),
            prefs_path,
            anchor: Mutex::new(None),
            moves: AtomicU64::new(0),
        }
    }

    /// Changes the prefs and writes them out. A failed write is logged and otherwise ignored.
    fn update_prefs(&self, change: impl FnOnce(&mut WidgetPrefs)) {
        let Ok(mut prefs) = self.prefs.lock() else {
            return;
        };
        let before = prefs.clone();
        change(&mut prefs);
        if *prefs == before {
            return;
        }
        if let Some(path) = &self.prefs_path {
            if let Err(e) = prefs.save(path) {
                eprintln!("could not save {}: {e}", path.display());
            }
        }
    }

    fn anchor(&self) -> Option<Anchor> {
        self.anchor.lock().ok().and_then(|a| *a)
    }

    fn set_anchor(&self, anchor: Anchor) {
        if let Ok(mut a) = self.anchor.lock() {
            *a = Some(anchor);
        }
    }
}

fn to_rect(monitor: &tauri::Monitor) -> Rect {
    let (pos, size) = (monitor.position(), monitor.size());
    Rect {
        x: pos.x,
        y: pos.y,
        width: i32::try_from(size.width).unwrap_or(i32::MAX),
        height: i32::try_from(size.height).unwrap_or(i32::MAX),
    }
}

/// All monitors, and the primary one (the first one if the system names none).
fn monitors(window: &Window) -> Option<(Vec<Rect>, Rect)> {
    let rects: Vec<Rect> = window
        .available_monitors()
        .ok()?
        .iter()
        .map(to_rect)
        .collect();
    let primary = window
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| to_rect(&m))
        .or_else(|| rects.first().copied())?;
    Some((rects, primary))
}

fn physical_width(window: &Window, logical: f64) -> i32 {
    let scale = window.scale_factor().unwrap_or(1.0);
    (logical * scale).round() as i32
}

/// Moves the window so its centre is at the anchor, hanging from the top edge.
fn place(window: &Window, anchor: Anchor, logical_width: f64) {
    let x = notch::anchored_x(anchor.center_x, physical_width(window, logical_width));
    if let Err(e) = window.set_position(PhysicalPosition::new(x, anchor.top_y)) {
        eprintln!("could not position the window: {e}");
    }
}

/// Resizes between the folded and unfolded pill, keeping the same top-centre point.
#[tauri::command]
fn set_size(window: Window, state: State<'_, NotchState>, folded: bool) -> Result<(), String> {
    let (width, height) = notch::size_for(folded);
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|e| format!("Could not resize the window: {e}"))?;
    if let Some(anchor) = state.anchor() {
        place(&window, anchor, width);
    }
    Ok(())
}

/// Puts the window at the saved (or default) spot. Runs once, before the window is shown.
fn init_position(window: &Window, state: &NotchState) {
    let Some((rects, primary)) = monitors(window) else {
        eprintln!("no monitor information; leaving the window where the system put it");
        return;
    };
    let saved = state.prefs.lock().ok().and_then(|p| p.center_x);
    let center_x = notch::resolve_center(saved, &rects, primary);
    let anchor = Anchor {
        center_x,
        top_y: notch::top_y_for(center_x, &rects, primary),
    };
    state.set_anchor(anchor);
    place(window, anchor, notch::FOLDED_SIZE.0);
}

/// A drag ends when the window has stopped moving for `DRAG_SETTLE`.
fn on_moved(window: &Window) {
    let seq = window
        .state::<NotchState>()
        .moves
        .fetch_add(1, Ordering::SeqCst)
        + 1;
    let window = window.clone();
    thread::spawn(move || {
        thread::sleep(DRAG_SETTLE);
        if window.state::<NotchState>().moves.load(Ordering::SeqCst) == seq {
            settle(&window);
        }
    });
}

/// Snaps the pill back to the top edge, keeps its x, and remembers the new centre.
fn settle(window: &Window) {
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.outer_size()) else {
        return;
    };
    let Some((rects, primary)) = monitors(window) else {
        return;
    };
    let center_x = notch::center_from_x(pos.x, i32::try_from(size.width).unwrap_or(0));
    let top_y = notch::top_y_for(center_x, &rects, primary);
    let state = window.state::<NotchState>();
    state.set_anchor(Anchor { center_x, top_y });
    if pos.y != top_y {
        if let Err(e) = window.set_position(PhysicalPosition::new(pos.x, top_y)) {
            eprintln!("could not snap the window to the top edge: {e}");
        }
    }
    state.update_prefs(|p| p.center_x = Some(center_x));
}

fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(WINDOW_LABEL) {
        let _ = window.hide();
    }
}

fn toggle_window(app: &AppHandle) {
    let visible = app
        .get_webview_window(WINDOW_LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_window(app);
    } else {
        show_window(app);
    }
}

/// Ctrl+Alt+R: stops a recording, or starts one when nothing is running. While the notes are
/// being written nothing happens, so a stray press cannot start a second run.
fn toggle_recording(app: &AppHandle) {
    let state = Arc::clone(&*app.state::<Arc<RunState>>());
    if state.request_stop().is_ok() {
        return;
    }
    if let Err(e) = launch(app, &state, Job::Record) {
        eprintln!("record hotkey: {e}");
    }
}

/// Ctrl+Alt+M (Control+Option+M on macOS) shows/hides the pill; Ctrl+Alt+R starts/stops a
/// recording. The plugin is added here, not on the builder, because its own setup fails when
/// there is no X11 connection (Wayland without XWayland), and a failed plugin setup would stop
/// the whole app. A failure here is only logged.
fn register_hotkeys(app: &AppHandle) {
    let toggle = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyM);
    let record = Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyR);
    let plugin = tauri_plugin_global_shortcut::Builder::new()
        .with_handler(move |app, pressed, event| {
            if event.state() != ShortcutState::Pressed {
                return;
            }
            if *pressed == toggle {
                toggle_window(app);
            } else if *pressed == record {
                toggle_recording(app);
            }
        })
        .build();
    if let Err(e) = app.plugin(plugin) {
        eprintln!("the hotkeys are off: {e}");
        return;
    }
    for (shortcut, name) in [(toggle, "Ctrl+Alt+M"), (record, "Ctrl+Alt+R")] {
        if let Err(e) = app.global_shortcut().register(shortcut) {
            eprintln!("the {name} hotkey is off: {e}");
        }
    }
}

/// Switches start-at-login on the first run only; after that the tray item is the user's choice.
fn first_run_autostart(app: &AppHandle) {
    let state = app.state::<NotchState>();
    if state.prefs.lock().map(|p| p.autostart_set).unwrap_or(true) {
        return;
    }
    match app.autolaunch().enable() {
        Ok(()) => state.update_prefs(|p| p.autostart_set = true),
        Err(e) => eprintln!("could not turn on start at login: {e}"),
    }
}

fn autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<tauri::Wry>) {
    let launch = app.autolaunch();
    let result = if autostart_enabled(app) {
        launch.disable()
    } else {
        launch.enable()
    };
    if let Err(e) = result {
        eprintln!("could not change start at login: {e}");
    }
    // Show what is really set, whether or not the change worked.
    let _ = item.set_checked(autostart_enabled(app));
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide", true, None::<&str>)?;
    let login = CheckMenuItem::with_id(
        app,
        "autostart",
        "Start at login",
        true,
        autostart_enabled(app),
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &login, &quit])?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Meeting Notes")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "hide" => hide_window(app),
            "autostart" => toggle_autostart(app, &login),
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
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(Arc::new(RunState::default()))
        .manage(NotchState::load())
        .invoke_handler(tauri::generate_handler![
            start,
            stop,
            retry,
            open_meetings,
            set_size
        ])
        .setup(|app| {
            let handle = app.handle();
            if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                init_position(&window.as_ref().window(), &handle.state::<NotchState>());
            }
            // The window starts hidden (tauri.conf.json) so it never flashes at the wrong spot.
            show_window(handle);
            first_run_autostart(handle);
            build_tray(handle)?;
            register_hotkeys(handle);
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing hides to the tray; "Quit" in the tray menu is the only way out.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Moved(_) => on_moved(window),
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running the Meeting Notes app");
}
