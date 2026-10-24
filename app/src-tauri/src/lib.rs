mod desktop_entry;
mod meetings;
mod notch;
mod queue;
mod runner;
mod self_update;
mod update;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
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
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use notch::{Anchor, Edge, Rect, WidgetPrefs};
use runner::RunState;

/// The recording, as the UI listens to it. The payload is the core's `Status` as JSON.
const STATUS_EVENT: &str = "mn-status";
/// The notes queue. Payload is `{ processing, waiting, last_error }` (see `QueueSnapshot`).
const QUEUE_EVENT: &str = "mn-queue";
/// A newer GitHub release. Payload is `{ version, prompt }`.
const UPDATE_EVENT: &str = "mn-update";
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

fn emit_status(app: &AppHandle, status: Status) {
    if let Err(e) = app.emit(STATUS_EVENT, &status) {
        eprintln!("could not send status to the window: {e}");
    }
}

/// Tells the window how the notes queue stands. Never touches the recording status.
fn emit_queue(app: &AppHandle, state: &RunState) {
    if let Err(e) = app.emit(QUEUE_EVENT, state.queue.snapshot()) {
        eprintln!("could not send the queue to the window: {e}");
    }
}

/// `{:#}` on an anyhow error prints the whole cause chain on one line.
fn describe(err: impl std::fmt::Display) -> String {
    format!("{err:#}")
}

/// Records until Stop. Returns the session dir.
fn record_session(app: &AppHandle, state: &RunState) -> Result<PathBuf, String> {
    let cfg = Config::load(None).map_err(describe)?;
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
        emit_status(app, status);
    };
    let recorded = mn_core::record(&cfg, &opts, on_status);
    state.set_recording(false);
    recorded.map_err(describe)
}

/// Transcribes, summarizes and publishes one session. Each stage is shown through the queue.
fn process_session(app: &AppHandle, state: &RunState, session: &Path) -> Result<(), String> {
    let cfg = Config::load(None).map_err(describe)?;
    let on_status = |status: Status| {
        if let Some(label) = queue::stage_label(&status) {
            state.queue.set_label(label);
            emit_queue(app, state);
        }
    };
    let result = mn_core::process(&cfg, session, on_status).map_err(describe);
    // The widget only shows the start of the error, so the full text goes to widget.log.
    if let Err(message) = &result {
        log(&format!(
            "notes failed for {}: {message}",
            session.display()
        ));
    }
    result
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

/// Starts a recording on a background thread. Errors only if a recording is already active; notes
/// being written for earlier meetings do not matter. When the recording stops, its session goes
/// to the back of the notes queue and the slot is free at once.
fn start_recording(app: &AppHandle, state: &Arc<RunState>) -> Result<(), String> {
    if app.state::<UpdateOffer>().updating.load(Ordering::SeqCst) {
        return Err(self_update::UPDATING.to_string());
    }
    let guard = state.begin()?;
    let app = app.clone();
    let state = Arc::clone(state);
    thread::Builder::new()
        .name("mn-run".into())
        .spawn(move || {
            let result = catch_unwind(AssertUnwindSafe(|| record_session(&app, &state)))
                .unwrap_or_else(|_| Err("The app hit an unexpected problem.".to_string()));
            state.set_recording(false);
            match result {
                Ok(session) => {
                    state.queue.push(session);
                    drop(guard);
                    emit_queue(&app, &state);
                }
                Err(message) => {
                    // Free the slot before the error status, so "Try again" never races the guard.
                    drop(guard);
                    log(&format!("recording failed: {message}"));
                    emit_status(&app, Status::Error { message });
                }
            }
        })
        .map(|_| ())
        .map_err(|e| format!("Could not start: {e}"))
}

/// The one thread that writes the notes: it takes queued sessions in order, one at a time. A
/// failed session is kept for "Try again" and the next one still runs.
fn spawn_queue_worker(app: AppHandle, state: Arc<RunState>) {
    let built = thread::Builder::new()
        .name("mn-queue".into())
        .spawn(move || loop {
            state.queue.wait_for_work();
            state.queue.work_one(
                |session| process_session(&app, &state, session),
                || emit_queue(&app, &state),
            );
        });
    if let Err(e) = built {
        eprintln!("could not start the notes queue: {e}");
    }
}

/// Queues the sessions an earlier run left unfinished (the app was closed, or the machine shut
/// down, while the notes were being written).
fn recover_unfinished(state: &RunState) {
    let cfg = match Config::load(None) {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("restart recovery: {}", describe(e));
            return;
        }
    };
    // Nothing is recording at launch, so there is no session to skip.
    let sessions = queue::unfinished_sessions(&cfg.notes_dir, None);
    if !sessions.is_empty() {
        log(&format!(
            "restart recovery: {} unfinished session(s)",
            sessions.len()
        ));
    }
    for session in sessions {
        state.queue.push(session);
    }
}

#[tauri::command]
fn start(app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    start_recording(&app, &state)
}

#[tauri::command]
fn stop(state: Shared<'_>) -> Result<(), String> {
    state.request_stop()
}

/// "Try again": queues the sessions whose notes failed. If none failed, the recording itself
/// failed, so this starts a fresh recording instead.
#[tauri::command]
fn retry(app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    if app.state::<UpdateOffer>().updating.load(Ordering::SeqCst) {
        return Err(self_update::UPDATING.to_string());
    }
    if state.queue.retry_failed() > 0 {
        emit_queue(&app, &state);
        return Ok(());
    }
    start_recording(&app, &state)
}

/// The queue as it is now, for a window that loaded after the last `mn-queue` event.
#[tauri::command]
fn queue_status(state: Shared<'_>) -> queue::QueueSnapshot {
    state.queue.snapshot()
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

/// Sets the size and turns resizing off, so the edges neither resize nor show resize cursors.
/// Linux: a non-resizable GTK window takes its size request and ignores `set_size` (it stays at
/// 200x200), so the request is set to the size itself.
#[cfg(target_os = "linux")]
fn lock_size(window: &Window, size: LogicalSize<f64>) -> tauri::Result<()> {
    let w = window.clone();
    window.run_on_main_thread(move || {
        use gtk::prelude::{GtkWindowExt, WidgetExt};
        if let Ok(gtk_window) = w.gtk_window() {
            let (width, height) = (size.width.round() as i32, size.height.round() as i32);
            gtk_window.set_size_request(width, height);
            gtk_window.set_resizable(false);
            gtk_window.resize(width, height);
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn lock_size(window: &Window, size: LogicalSize<f64>) -> tauri::Result<()> {
    window.set_size(size)?;
    window.set_resizable(false)
}

/// Resizes between the folded tab and the unfolded column, keeping the same edge and the same
/// vertical centre.
#[tauri::command]
fn set_size(window: Window, state: State<'_, NotchState>, folded: bool) -> Result<(), String> {
    let (width, height) = notch::size_for(folded);
    state.folded.store(folded, Ordering::SeqCst);
    lock_size(&window, LogicalSize::new(width, height))
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
    let home = notch::home_monitor(&rects, primary, cfg!(target_os = "linux"));
    state.set_anchor(notch::resolve(saved, &rects, home));
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

/// Ctrl+Alt+R: stops a recording, or starts one when nothing is recording (notes being written
/// for an earlier meeting do not matter).
fn toggle_recording(app: &AppHandle) {
    let state = Arc::clone(&*app.state::<Arc<RunState>>());
    if state.request_stop().is_ok() {
        return;
    }
    if let Err(e) = start_recording(app, &state) {
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

/// Linux: the autostart plugin writes `Exec=` with no quotes, so a path with spaces never
/// starts. Quote it, and point an AppImage at `$APPIMAGE` rather than the temporary mount.
/// No-op on Windows and macOS.
#[cfg(target_os = "linux")]
fn fix_linux_autostart(app: &AppHandle) {
    let Some(home) = dirs::home_dir() else {
        eprintln!("could not fix start at login: no home directory");
        return;
    };
    let file = desktop_entry::autostart_desktop_path(&home, &app.package_info().name);
    let current = match std::env::current_exe() {
        Ok(path) => path.display().to_string(),
        Err(err) => {
            eprintln!("could not fix start at login: {err}");
            return;
        }
    };
    let appimage = std::env::var("APPIMAGE").ok();
    let launch = desktop_entry::launch_target(appimage.as_deref(), &current);
    if let Err(err) = desktop_entry::rewrite_autostart_file(&file, &launch) {
        eprintln!("could not fix start at login: {err}");
    }
}

#[cfg(not(target_os = "linux"))]
fn fix_linux_autostart(_app: &AppHandle) {}

fn autostart_enabled(app: &AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

fn toggle_autostart(app: &AppHandle, item: &CheckMenuItem<tauri::Wry>) {
    let launch = app.autolaunch();
    let turning_on = !autostart_enabled(app);
    let result = if turning_on {
        launch.enable()
    } else {
        launch.disable()
    };
    if let Err(e) = result {
        eprintln!("could not change start at login: {e}");
    } else if turning_on {
        fix_linux_autostart(app);
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
            "update" => begin_update(app, true),
            "quit" => {
                let app = app.clone();
                let state = Arc::clone(&*app.state::<Arc<RunState>>());
                // Close the WAV files first, off the UI thread. Notes being written are not
                // waited for: the app just exits, and the next launch finds the sessions that
                // have audio or a transcript but no card.json and queues them again
                // (`recover_unfinished`).
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
    if let Ok(mut slot) = app.state::<UpdateOffer>().menu.lock() {
        *slot = Some(menu);
    }
    Ok(())
}

/// The release the tray and the widget are currently offering, if any.
struct UpdateOffer {
    prompt: Mutex<String>,
    version: Mutex<String>,
    menu: Mutex<Option<Menu<tauri::Wry>>>,
    item: Mutex<Option<MenuItem<tauri::Wry>>>,
    /// An updater script is running. Start stays disabled until it finishes or the app restarts.
    updating: AtomicBool,
    /// One poll thread at a time.
    polling: AtomicBool,
}

impl Default for UpdateOffer {
    fn default() -> Self {
        Self {
            prompt: Mutex::new(String::new()),
            version: Mutex::new(String::new()),
            menu: Mutex::new(None),
            item: Mutex::new(None),
            updating: AtomicBool::new(false),
            polling: AtomicBool::new(false),
        }
    }
}

#[derive(Clone, serde::Serialize)]
struct UpdatePayload {
    phase: String,
    version: String,
    prompt: String,
    message: String,
}

fn update_payload(phase: &str, version: &str, prompt: &str, message: &str) -> UpdatePayload {
    UpdatePayload {
        phase: phase.to_string(),
        version: version.to_string(),
        prompt: prompt.to_string(),
        message: message.to_string(),
    }
}

fn quiet_payload() -> UpdatePayload {
    update_payload("quiet", "", "", "")
}

/// Same fallback as the core config helper: `.` when the home directory cannot be found.
fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

fn repair_prompt() -> String {
    update::update_prompt_for_home(&home_dir())
}

/// `<config dir>/meeting-notes/update-status.json`.
fn update_status_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| self_update::status_file(&self_update::app_config_dir(&dir)))
}

fn emit_update(app: &AppHandle, payload: &UpdatePayload) {
    if let Err(err) = app.emit(UPDATE_EVENT, payload) {
        eprintln!("could not send the update to the window: {err}");
    }
}

fn notify(app: &AppHandle, body: &str) {
    let shown = app
        .notification()
        .builder()
        .title("Meeting Notes")
        .body(body)
        .show();
    if let Err(err) = shown {
        eprintln!("could not show a notification: {err}");
    }
}

/// Adds (or renames) the tray item and tells the widget. Errors are logged; the app keeps running.
fn announce_update(app: &AppHandle, version: &str, prompt: &str) {
    let state = app.state::<UpdateOffer>();
    if let Ok(mut slot) = state.prompt.lock() {
        *slot = prompt.to_string();
    }
    if let Ok(mut slot) = state.version.lock() {
        *slot = version.to_string();
    }
    let label = self_update::tray_label(version);
    let menu = state.menu.lock().ok().and_then(|slot| slot.clone());
    match state.item.lock() {
        Ok(mut slot) => {
            if let Some(item) = slot.as_ref() {
                if let Err(err) = item.set_text(&label) {
                    eprintln!("could not rename the update tray item: {err}");
                }
            } else if let Some(menu) = menu {
                match MenuItem::with_id(app, "update", &label, true, None::<&str>) {
                    Ok(item) => {
                        // Show, Hide, Start at login, then this, then Quit.
                        if let Err(err) = menu.insert(&item, 3) {
                            eprintln!("could not add the update tray item: {err}");
                        } else {
                            *slot = Some(item);
                        }
                    }
                    Err(err) => eprintln!("could not create the update tray item: {err}"),
                }
            } else {
                eprintln!("update check: the tray menu is not ready");
            }
        }
        Err(_) => eprintln!("update check: the tray state is locked"),
    }
    log(&format!("update available {version}"));
    if state.updating.load(Ordering::SeqCst) {
        return;
    }
    emit_update(
        app,
        &update_payload("offer", version, prompt, &self_update::tray_label(version)),
    );
}

fn spawn_update_check(app: AppHandle) {
    let built = thread::Builder::new()
        .name("mn-update".into())
        .spawn(move || {
            thread::sleep(update::FIRST_CHECK);
            loop {
                match update::latest_tag() {
                    Ok(tag) => {
                        let current = app.package_info().version.to_string();
                        if update::is_newer(&current, &tag) {
                            if let Some(version) = update::display_version(&tag) {
                                let prompt = update::update_prompt_for_home(&home_dir());
                                announce_update(&app, &version, &prompt);
                            }
                        }
                    }
                    Err(err) => eprintln!("update check: {err}"),
                }
                thread::sleep(update::CHECK_EVERY);
            }
        });
    if let Err(err) = built {
        eprintln!("could not start the update check: {err}");
    }
}

/// Button or tray: start the updater, or copy the repair sentence when the script is missing.
fn begin_update(app: &AppHandle, from_tray: bool) {
    let offer = app.state::<UpdateOffer>();
    if offer.updating.load(Ordering::SeqCst) {
        // The widget is already in the updating state. A tray click only needs a reminder.
        if from_tray {
            notify(app, self_update::UPDATING);
        }
        return;
    }
    let run = app.state::<Arc<RunState>>();
    let snap = run.queue.snapshot();
    let pending = self_update::notes_pending(snap.processing.is_some(), snap.waiting);
    if let Some(message) = self_update::refusal(run.is_recording(), pending) {
        tell_update(app, from_tray, "refused", message);
        return;
    }
    let repo = self_update::repo_dir(&home_dir());
    let host = self_update::host();
    let script = self_update::updater_script(&repo, host);
    if !script.is_file() {
        log("update: no updater script, copying the repair sentence");
        copy_repair(app, from_tray);
        return;
    }
    let Some(status) = update_status_path() else {
        eprintln!("update: no config directory, copying the repair sentence");
        copy_repair(app, from_tray);
        return;
    };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let work = self_update::fresh_work_dir(&std::env::temp_dir(), nonce);
    let unix = self_update::detach_for_path(&std::env::var("PATH").unwrap_or_default());
    let spec = match self_update::prepare_launch(&script, &repo, &status, &work, host, unix) {
        Ok(spec) => spec,
        Err(err) => {
            eprintln!("update: {err}");
            tell_update(
                app,
                from_tray,
                "failed",
                "Update failed. The updater could not be started.",
            );
            return;
        }
    };
    if let Err(err) = spawn_detached(&spec) {
        eprintln!("update: {err}");
        tell_update(
            app,
            from_tray,
            "failed",
            "Update failed. The updater could not be started.",
        );
        return;
    }
    offer.updating.store(true, Ordering::SeqCst);
    log(&format!("update: started {}", spec.program));
    let prompt = stored_prompt(app);
    emit_update(
        app,
        &update_payload("updating", "", &prompt, self_update::UPDATING),
    );
    if from_tray {
        notify(app, self_update::UPDATING);
    }
    spawn_status_poll(app.clone(), status, false);
}

fn tell_update(app: &AppHandle, from_tray: bool, phase: &str, message: &str) {
    let prompt = stored_prompt(app);
    emit_update(app, &update_payload(phase, "", &prompt, message));
    if from_tray {
        notify(app, message);
    }
}

fn stored_prompt(app: &AppHandle) -> String {
    app.state::<UpdateOffer>()
        .prompt
        .lock()
        .ok()
        .filter(|text| !text.is_empty())
        .map(|text| text.clone())
        .unwrap_or_else(repair_prompt)
}

/// Old or broken install: the script is not in the clone, so copy the repair sentence.
fn copy_repair(app: &AppHandle, from_tray: bool) {
    let prompt = repair_prompt();
    if let Ok(mut slot) = app.state::<UpdateOffer>().prompt.lock() {
        *slot = prompt.clone();
    }
    if let Err(err) = app.clipboard().write_text(&prompt) {
        eprintln!("could not copy the update prompt: {err}");
        tell_update(app, from_tray, "failed", "Could not copy the update steps.");
        return;
    }
    emit_update(
        app,
        &update_payload("copied", "", &prompt, self_update::COPY_FALLBACK),
    );
    if from_tray {
        notify(app, self_update::COPY_FALLBACK);
    }
}

/// `announce_done` is for the app the setup script relaunched while the status was still running.
fn spawn_status_poll(app: AppHandle, path: PathBuf, announce_done: bool) {
    if app
        .state::<UpdateOffer>()
        .polling
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let worker = app.clone();
    let built = thread::Builder::new()
        .name("mn-update-poll".into())
        .spawn(move || {
            let deadline = Instant::now() + self_update::POLL_FOR;
            loop {
                thread::sleep(self_update::POLL_EVERY);
                if Instant::now() >= deadline {
                    eprintln!("update: no finished status after 15 minutes");
                    break;
                }
                let status = match self_update::read_status(&path) {
                    Ok(Some(status)) => status,
                    Ok(None) => continue,
                    Err(err) => {
                        eprintln!("update status: {err}");
                        continue;
                    }
                };
                match self_update::watch_step(&status, announce_done) {
                    self_update::PollOutcome::Wait => {}
                    self_update::PollOutcome::UpToDate => {
                        if let Err(err) = self_update::mark_seen(&path) {
                            eprintln!("update status: {err}");
                        }
                        finish_update_wait(&worker, "up_to_date", self_update::ALREADY_NEWEST);
                        break;
                    }
                    self_update::PollOutcome::Failed { message } => {
                        finish_update_wait(&worker, "failed", &message);
                        break;
                    }
                    self_update::PollOutcome::Updated { version } => {
                        let message = self_update::updated_message(&version);
                        notify(&worker, &message);
                        if let Err(err) = self_update::mark_seen(&path) {
                            eprintln!("update status: {err}");
                        }
                        finish_update_wait(&worker, "quiet", "");
                        break;
                    }
                }
            }
            worker
                .state::<UpdateOffer>()
                .polling
                .store(false, Ordering::SeqCst);
        });
    if let Err(err) = built {
        app.state::<UpdateOffer>()
            .polling
            .store(false, Ordering::SeqCst);
        eprintln!("could not watch the update: {err}");
    }
}

fn finish_update_wait(app: &AppHandle, phase: &str, message: &str) {
    app.state::<UpdateOffer>()
        .updating
        .store(false, Ordering::SeqCst);
    let prompt = stored_prompt(app);
    emit_update(app, &update_payload(phase, "", &prompt, message));
}

/// Starts the copied script in its own session so it survives this app quitting.
fn spawn_detached(spec: &self_update::SpawnSpec) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::{Command, Stdio};
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // nohup does not start a new session. setsid does, and it fails if the process is
        // already a process-group leader, so only the nohup path asks for a new group.
        if spec.own_process_group {
            cmd.process_group(0);
        }
        let mut child = cmd
            .spawn()
            .map_err(|err| format!("Could not start the update: {err}"))?;
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        let _ = spec.own_process_group;
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(self_update::windows_creation_flags());
        let mut child = cmd
            .spawn()
            .map_err(|err| format!("Could not start the update: {err}"))?;
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

/// First look at the status file after the window is up. `done` notifies and marks seen.
#[tauri::command]
fn startup_update(app: AppHandle) -> UpdatePayload {
    let Some(path) = update_status_path() else {
        return quiet_payload();
    };
    let status = match self_update::read_status(&path) {
        Ok(Some(status)) => status,
        Ok(None) => return quiet_payload(),
        Err(err) => {
            eprintln!("update status: {err}");
            return quiet_payload();
        }
    };
    let prompt = repair_prompt();
    match self_update::notice_on_start(&status, chrono::Utc::now()) {
        self_update::StartNotice::Quiet => quiet_payload(),
        self_update::StartNotice::Updated { version } => {
            let message = self_update::updated_message(&version);
            notify(&app, &message);
            if let Err(err) = self_update::mark_seen(&path) {
                eprintln!("update status: {err}");
            }
            quiet_payload()
        }
        self_update::StartNotice::Failed { message } => {
            update_payload("failed", "", &prompt, &message)
        }
        self_update::StartNotice::InProgress => {
            app.state::<UpdateOffer>()
                .updating
                .store(true, Ordering::SeqCst);
            spawn_status_poll(app, path, true);
            update_payload("updating", "", &prompt, self_update::UPDATING)
        }
    }
}

/// Widget button or the follow-up from a tray click's event. Refusals are events, not errors.
#[tauri::command]
fn start_update(app: AppHandle) {
    begin_update(&app, false);
}

/// The failed-update note was dismissed. Later launches stay quiet.
#[tauri::command]
fn dismiss_update_notice() -> Result<(), String> {
    if let Some(path) = update_status_path() {
        self_update::mark_seen(&path)?;
    }
    Ok(())
}

/// "Get help": copy the repair sentence, then mark the failure seen.
#[tauri::command]
fn copy_update_help(app: AppHandle) -> Result<(), String> {
    let prompt = repair_prompt();
    app.clipboard()
        .write_text(&prompt)
        .map_err(|err| format!("Could not copy: {err}"))?;
    if let Some(path) = update_status_path() {
        if let Err(err) = self_update::mark_seen(&path) {
            eprintln!("update status: {err}");
        }
    }
    emit_update(
        &app,
        &update_payload("copied", "", &prompt, self_update::COPY_HELP),
    );
    Ok(())
}

/// Fallback when the webview clipboard refuses the write. The button click is the gesture.
#[tauri::command]
fn copy_update_prompt(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|err| format!("Could not copy: {err}"))
}

pub fn run() {
    tauri::Builder::default()
        // First plugin: a second launch exits and this callback runs in the copy that is
        // already running.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            log("second launch: showing the widget");
            show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(Arc::new(RunState::default()))
        .manage(NotchState::load())
        .manage(UpdateOffer::default())
        .invoke_handler(tauri::generate_handler![
            start,
            stop,
            retry,
            queue_status,
            open_meetings,
            set_size,
            current_edge,
            selftest_enabled,
            start_drag,
            copy_update_prompt,
            start_update,
            startup_update,
            dismiss_update_notice,
            copy_update_help
        ])
        .setup(|app| {
            let handle = app.handle();
            if let Some(window) = handle.get_webview_window(WINDOW_LABEL) {
                init_position(&window.as_ref().window(), &handle.state::<NotchState>());
            }
            // The window starts hidden (tauri.conf.json) so it never flashes at the wrong spot.
            show_window(handle);
            first_run_autostart(handle);
            // Quote the Linux login entry on every start, including after a moved AppImage.
            if autostart_enabled(handle) {
                fix_linux_autostart(handle);
            }
            build_tray(handle)?;
            spawn_update_check(handle.clone());
            register_hotkeys(handle);
            let run_state = Arc::clone(&*handle.state::<Arc<RunState>>());
            recover_unfinished(&run_state);
            spawn_queue_worker(handle.clone(), run_state);
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
