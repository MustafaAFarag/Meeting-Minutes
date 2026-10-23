mod meetings;
mod notch;
mod runner;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mn_core::record::RecordOptions;
use mn_core::{Config, Status};
use tauri::menu::{CheckMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, Window, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use notch::{Anchor, Edge, Rect, WidgetPrefs};
use runner::RunState;

/// The event the UI listens to. The payload is the core's `Status` as JSON.
const STATUS_EVENT: &str = "mn-status";
/// Tells the UI which edge the tab hangs from ("left" or "right") after a drop.
const EDGE_EVENT: &str = "mn-edge";
const WINDOW_LABEL: &str = "main";
/// After the window stops moving for this long, a drag counts as finished.
const DRAG_SETTLE: Duration = Duration::from_millis(250);
/// `Moved` events this soon after we moved the window ourselves are not a drag.
const OWN_MOVE_QUIET: Duration = Duration::from_millis(300);
/// How long a resize gets before we check the window really is where it belongs.
const RESIZE_SETTLE: Duration = Duration::from_millis(150);

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
            let every = runner::remind_every();
            let reminders = AtomicU64::new(0);
            let on_status = |status: Status| {
                if let Status::Recording { elapsed, .. } = &status {
                    let sent = reminders.load(Ordering::SeqCst);
                    if let Some(n) = runner::reminder_due(*elapsed, sent, every) {
                        reminders.store(n, Ordering::SeqCst);
                        remind_still_recording(app, *elapsed / 60);
                    }
                }
                emit(status);
            };
            let recorded = mn_core::record(&cfg, &opts, on_status);
            state.set_recording(false);
            let dir = recorded.map_err(describe)?;
            state.set_last_session(Some(dir.clone()));
            dir
        }
        Job::Process(dir) => dir,
    };
    mn_core::process(&cfg, &session, emit).map_err(describe)
}

/// A system notification that the recording is still running. Failures are only logged; the
/// recording carries on either way.
fn remind_still_recording(app: &AppHandle, minutes: u64) {
    log(&format!("reminder: still recording after {minutes} min"));
    let shown = app
        .notification()
        .builder()
        .title("Still recording")
        .body(format!(
            "Meeting Notes has been recording for {minutes} minutes. Press Stop when the meeting is over."
        ))
        .show();
    if let Err(e) = shown {
        log(&format!("could not show the reminder: {e}"));
    }
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

struct NotchState {
    prefs: Mutex<WidgetPrefs>,
    prefs_path: Option<PathBuf>,
    anchor: Mutex<Option<Anchor>>,
    /// Whether the window is the folded tab (true) or the unfolded column. Kept here so the
    /// placement maths never depends on reading a size back from a window that is still resizing.
    folded: AtomicBool,
    /// A drag was started and has not been dropped yet.
    dragging: AtomicBool,
    /// The user hid the window (hotkey, tray, close). A resize must not bring it back then.
    user_hidden: AtomicBool,
    /// Moves we made ourselves are not drags: `Moved` events before this moment are ignored.
    own_move_until: Mutex<Option<Instant>>,
    /// Counts window moves, so only the last one of a drag is acted on.
    moves: AtomicU64,
    /// Counts resizes, so only the last one is double-checked.
    resizes: AtomicU64,
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
            folded: AtomicBool::new(false),
            dragging: AtomicBool::new(false),
            user_hidden: AtomicBool::new(false),
            own_move_until: Mutex::new(None),
            moves: AtomicU64::new(0),
            resizes: AtomicU64::new(0),
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

    fn mark_own_move(&self) {
        if let Ok(mut until) = self.own_move_until.lock() {
            *until = Some(Instant::now() + OWN_MOVE_QUIET);
        }
    }

    fn clear_own_move(&self) {
        if let Ok(mut until) = self.own_move_until.lock() {
            *until = None;
        }
    }

    fn is_own_move(&self) -> bool {
        self.own_move_until
            .lock()
            .ok()
            .and_then(|until| *until)
            .is_some_and(|until| Instant::now() < until)
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

/// Physical size of the window in its current (folded or unfolded) state.
fn physical_size(window: &Window, folded: bool) -> (i32, i32) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let (w, h) = notch::size_for(folded);
    ((w * scale).round() as i32, (h * scale).round() as i32)
}

/// Where the window's top-left corner belongs for the current anchor and fold state.
fn target_position(window: &Window, state: &NotchState) -> Option<(i32, i32)> {
    let anchor = state.anchor()?;
    let (w, h) = physical_size(window, state.folded.load(Ordering::SeqCst));
    Some(anchor.window_pos(w, h))
}

/// Moves the window to its anchored spot, hanging from the edge.
fn place(window: &Window, state: &NotchState) {
    let Some((x, y)) = target_position(window, state) else {
        return;
    };
    state.mark_own_move();
    if let Err(e) = window.set_position(PhysicalPosition::new(x, y)) {
        eprintln!("could not position the window: {e}");
    }
}

fn selftest_on() -> bool {
    std::env::var("MN_NOTCH_SELFTEST").is_ok_and(|v| v == "1")
}

/// Self-test only: what the window really reports. `label` is "" right after a resize and
/// "settled " once the window had time to take it.
fn report(window: &Window, label: &str) {
    if !selftest_on() {
        return;
    }
    let size = window
        .outer_size()
        .map(|s| format!("{}x{}", s.width, s.height))
        .unwrap_or_else(|_| "?".into());
    let pos = window
        .outer_position()
        .map(|p| format!("{},{}", p.x, p.y))
        .unwrap_or_else(|_| "?".into());
    let visible = window.is_visible().unwrap_or(false);
    eprintln!("NOTCH {label}size={size} pos={pos} visible={visible}");
}

/// Resizing is asynchronous on GTK, so look again shortly after: if the window is not where it
/// belongs, move it there, and if it is hidden without the user asking for that, show it.
fn ensure_placed(window: &Window) {
    let state = window.state::<NotchState>();
    if state.dragging.load(Ordering::SeqCst) || state.user_hidden.load(Ordering::SeqCst) {
        return;
    }
    if let (Some(want), Ok(have)) = (target_position(window, &state), window.outer_position()) {
        if (have.x, have.y) != want {
            place(window, &state);
        }
    }
    if !state.user_hidden.load(Ordering::SeqCst) && !window.is_visible().unwrap_or(true) {
        let _ = window.show();
    }
    report(window, "settled ");
}

/// Resizes between the folded tab and the unfolded column, keeping the same edge and the same
/// vertical centre.
#[tauri::command]
fn set_size(window: Window, state: State<'_, NotchState>, folded: bool) -> Result<(), String> {
    let (width, height) = notch::size_for(folded);
    state.folded.store(folded, Ordering::SeqCst);
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|e| format!("Could not resize the window: {e}"))?;
    if !state.dragging.load(Ordering::SeqCst) && !state.user_hidden.load(Ordering::SeqCst) {
        place(&window, &state);
    }
    if !state.user_hidden.load(Ordering::SeqCst) && !window.is_visible().unwrap_or(true) {
        let _ = window.show();
    }
    report(&window, "");
    let seq = state.resizes.fetch_add(1, Ordering::SeqCst) + 1;
    let window = window.clone();
    thread::spawn(move || {
        thread::sleep(RESIZE_SETTLE);
        if window.state::<NotchState>().resizes.load(Ordering::SeqCst) == seq {
            ensure_placed(&window);
        }
    });
    Ok(())
}

/// The edge the tab hangs from, so the UI can round the corners that face the screen.
#[tauri::command]
fn current_edge(state: State<'_, NotchState>) -> &'static str {
    state.anchor().map_or(Edge::Right, |a| a.edge).as_str()
}

/// Whether the UI should run its fold/unfold self-test (`MN_NOTCH_SELFTEST=1`).
#[tauri::command]
fn selftest_enabled() -> bool {
    selftest_on()
}

/// Starts a drag. The drop is noticed when the window stops moving (see `on_moved`).
#[tauri::command]
fn start_drag(window: Window, state: State<'_, NotchState>) -> Result<(), String> {
    state.dragging.store(true, Ordering::SeqCst);
    state.clear_own_move();
    window
        .start_dragging()
        .inspect_err(|_| state.dragging.store(false, Ordering::SeqCst))
        .map_err(|e| format!("Could not start the drag: {e}"))
}

fn emit_edge(window: &Window, edge: Edge) {
    if let Err(e) = window.emit(EDGE_EVENT, edge.as_str()) {
        eprintln!("could not send the edge to the window: {e}");
    }
}

/// Puts the window at the saved (or default) spot. Runs once, before the window is shown.
fn init_position(window: &Window, state: &NotchState) {
    let Some((rects, primary)) = monitors(window) else {
        eprintln!("no monitor information; leaving the window where the system put it");
        return;
    };
    let saved = state.prefs.lock().ok().and_then(|p| p.pos);
    state.set_anchor(notch::resolve(saved, &rects, primary));
    place(window, state);
}

/// Our own moves (placing, snapping) are not drags and are ignored.
fn on_moved(window: &Window) {
    let state = window.state::<NotchState>();
    if state.is_own_move() || !state.dragging.load(Ordering::SeqCst) {
        return;
    }
    let seq = state.moves.fetch_add(1, Ordering::SeqCst) + 1;
    let window = window.clone();
    thread::spawn(move || {
        thread::sleep(DRAG_SETTLE);
        if window.state::<NotchState>().moves.load(Ordering::SeqCst) == seq {
            settle(&window);
        }
    });
}

/// The drop: snaps the tab to the nearest side edge of the monitor it was dropped on, and
/// remembers the spot.
fn settle(window: &Window) {
    let state = window.state::<NotchState>();
    let Ok(pos) = window.outer_position() else {
        return;
    };
    let Some((rects, primary)) = monitors(window) else {
        return;
    };
    let (w, h) = physical_size(window, state.folded.load(Ordering::SeqCst));
    let drop_x = pos.x + w / 2;
    let anchor = notch::drop_anchor(drop_x, pos.y + h / 2, &rects, primary);
    state.set_anchor(anchor);
    state.dragging.store(false, Ordering::SeqCst);
    place(window, &state);
    emit_edge(window, anchor.edge);
    let saved = anchor.saved(drop_x);
    state.update_prefs(|p| p.pos = Some(saved));
}

/// One line to stderr and to `<cache dir>/meeting-notes/widget.log`, so show/hide problems can be
/// traced even when the widget was started at login with no terminal.
fn log(msg: &str) {
    let line = format!(
        "{} {msg}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f")
    );
    eprintln!("{line}");
    if let Some(dir) = dirs::cache_dir().map(|d| d.join("meeting-notes")) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("widget.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }
}

/// What the window says about itself, for the log.
fn describe_window(window: &Window) -> String {
    let pos = window
        .outer_position()
        .map(|p| format!("{},{}", p.x, p.y))
        .unwrap_or_else(|_| "?".into());
    let size = window
        .outer_size()
        .map(|s| format!("{}x{}", s.width, s.height))
        .unwrap_or_else(|_| "?".into());
    let visible = window
        .is_visible()
        .map_or_else(|_| "?".into(), |v| v.to_string());
    format!("pos={pos} size={size} visible={visible}")
}

/// Linux: an unmapped webview is slow to redraw when it comes back, and KWin will not let the
/// window sit off screen. So "hidden" there means fully transparent and click-through, which is
/// instant and also invisible in a screen share. Elsewhere a real hide/show is already instant.
#[cfg(target_os = "linux")]
fn set_hidden(window: &Window, hidden: bool) {
    let w = window.clone();
    let _ = window.run_on_main_thread(move || {
        use gtk::prelude::WidgetExt;
        if let Ok(gtk_window) = w.gtk_window() {
            gtk_window.set_opacity(if hidden { 0.0 } else { 1.0 });
        }
        let _ = w.set_ignore_cursor_events(hidden);
    });
}

#[cfg(not(target_os = "linux"))]
fn set_hidden(window: &Window, hidden: bool) {
    let _ = if hidden { window.hide() } else { window.show() };
}

fn show_window(app: &AppHandle) {
    let Some(webview) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    let window = webview.as_ref().window();
    let state = app.state::<NotchState>();
    state.user_hidden.store(false, Ordering::SeqCst);
    // Unmapped only at startup (tauri.conf.json starts it hidden).
    if !window.is_visible().unwrap_or(false) {
        let _ = window.show();
    }
    set_hidden(&window, false);
    place(&window, &state);
    log(&format!("show: {}", describe_window(&window)));
    // Showing is asynchronous on X11: check again once it has happened.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        ensure_placed(&window);
        log(&format!("show settled: {}", describe_window(&window)));
    });
}

/// Hides the widget. Recording is not touched.
fn hide_window(app: &AppHandle) {
    let Some(webview) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    let window = webview.as_ref().window();
    app.state::<NotchState>()
        .user_hidden
        .store(true, Ordering::SeqCst);
    set_hidden(&window, true);
    log(&format!("hide: {}", describe_window(&window)));
}

/// Decides by our own flag, not by `is_visible()`: on X11 that answer lags behind a show or
/// hide, so a quick second press used to read the old state and do the wrong thing.
fn toggle_window(app: &AppHandle) {
    let hidden = app.state::<NotchState>().user_hidden.load(Ordering::SeqCst);
    log(&format!("Ctrl+Alt+M: hidden={hidden}"));
    if hidden {
        show_window(app);
    } else {
        hide_window(app);
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

/// Ctrl+Alt+M (Control+Option+M on macOS) shows/hides the tab; Ctrl+Alt+R starts/stops a
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
        .plugin(tauri_plugin_notification::init())
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
            set_size,
            current_edge,
            selftest_enabled,
            start_drag
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
            if selftest_on() {
                // No keyboard in the self-test: press "Ctrl+Alt+M" on a timer instead.
                let handle = handle.clone();
                thread::spawn(move || {
                    for _ in 0..6 {
                        thread::sleep(Duration::from_millis(1500));
                        toggle_window(&handle);
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| match event {
            // Closing hides to the tray; "Quit" in the tray menu is the only way out.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                hide_window(window.app_handle());
            }
            WindowEvent::Moved(_) => on_moved(window),
            _ => {}
        })
        .run(tauri::generate_context!())
        .expect("error while running the Meeting Notes app");
}
