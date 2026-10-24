mod applog;
mod card;
mod cards;
mod desktop_entry;
mod meetings;
mod menu;
mod notch;
mod queue;
mod runner;
mod self_update;
mod update;

use std::cell::Cell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::anyhow;
use mn_core::record::RecordOptions;
use mn_core::{Config, ErrorKind, Failure, Stage, Status};
use tauri::menu::{CheckMenuItem, IsMenuItem, Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, State, Window, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt as _};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use applog::{error, info, warn};
use card::{Action, MeetingsProblem, StartRefusal, Topic};
use cards::CardState;
use notch::{Anchor, Edge, Rect, WidgetPrefs};
use queue::Problem;
use runner::RunState;

/// The recording, as the UI listens to it. The payload is the core's `Status` as JSON.
const STATUS_EVENT: &str = "mn-status";
/// The notes queue. Payload is `{ processing, waiting, last_error, failed, ready }` (see
/// `QueueSnapshot`).
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
/// How long "Try a short test" listens.
const SOUND_TEST_SECS: u64 = 5;
/// The temporary folder of a sound test starts with this, and is removed afterwards.
const SOUND_TEST_PREFIX: &str = "meeting-notes-sound-test-";

type Shared<'a> = State<'a, Arc<RunState>>;

/// What a recording is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A real meeting: its session goes to the notes queue.
    Meeting,
    /// A few seconds to check the sound. Recorded into a temporary folder and thrown away.
    SoundTest,
}

fn emit_status(app: &AppHandle, status: Status) {
    if let Err(e) = app.emit(STATUS_EVENT, &status) {
        warn(&format!("could not send status to the window: {e}"));
    }
}

/// Tells the window how the notes queue stands. Never touches the recording status.
fn emit_queue(app: &AppHandle, state: &RunState) {
    if let Err(e) = app.emit(QUEUE_EVENT, state.queue.snapshot()) {
        warn(&format!("could not send the queue to the window: {e}"));
    }
    cards::refresh_working(app, state);
}

/// Where a sound test records. Unique per test, so nothing is ever mixed with real notes.
fn sound_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    std::env::temp_dir().join(format!("{SOUND_TEST_PREFIX}{}-{nanos}", std::process::id()))
}

/// True only for a folder `sound_test_dir` could have made: directly in the temp folder, with the
/// test prefix. Nothing else is ever removed.
fn is_sound_test_dir(dir: &Path, temp: &Path) -> bool {
    dir.parent() == Some(temp)
        && dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(SOUND_TEST_PREFIX))
}

fn remove_sound_test_dir(dir: &Path) {
    if !is_sound_test_dir(dir, &std::env::temp_dir()) {
        warn("sound test: refused to remove a folder that is not a test folder");
        return;
    }
    if let Err(err) = std::fs::remove_dir_all(dir) {
        if err.kind() != std::io::ErrorKind::NotFound {
            warn(&format!("sound test: could not remove its folder: {err}"));
        }
    }
}

/// Records until Stop (or, for a sound test, a few seconds). Returns the session dir.
fn record_session(
    app: &AppHandle,
    state: &RunState,
    test_dir: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let mut cfg =
        Config::load(None).map_err(|err| Failure::attach(err, ErrorKind::Config, Stage::Record))?;
    if let Some(dir) = test_dir {
        cfg.notes_dir = dir.to_path_buf();
    }
    state.set_recording(true);
    let opts = RecordOptions {
        max_seconds: test_dir.map(|_| SOUND_TEST_SECS),
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
    recorded
}

/// Logs a failure with its technical reason (log only) and returns the plain kind the card
/// explains.
fn problem_of(label: &str, err: &anyhow::Error, stage: Stage) -> Problem {
    let (kind, _, _) = mn_core::classify(err, stage);
    let at = err.downcast_ref::<Failure>().map_or(stage, |f| f.stage);
    error(&format!(
        "notes failed: session={label} kind={} stage={} detail={}",
        kind.as_str(),
        at.as_str(),
        applog::failure_detail(err)
    ));
    Problem { kind, stage: at }
}

/// Transcribes, summarizes and publishes one session. Each stage is shown through the queue.
fn process_session(app: &AppHandle, state: &RunState, session: &Path) -> Result<(), Problem> {
    let label = card::session_label(session);
    info(&format!("queue: take session={label}"));
    let cfg = Config::load(None).map_err(|err| {
        let err = Failure::attach(err, ErrorKind::Config, Stage::Other);
        problem_of(&label, &err, Stage::Other)
    })?;
    let stage = Cell::new(Stage::Other);
    let on_status = |status: Status| {
        if let Some(now) = queue::stage_of(&status) {
            stage.set(now);
        }
        if let Some(words) = queue::stage_label(&status) {
            info(&format!(
                "queue: session={label} stage={}",
                stage.get().as_str()
            ));
            state.queue.set_label(words);
            emit_queue(app, state);
        }
    };
    mn_core::process(&cfg, session, on_status).map_err(|err| problem_of(&label, &err, stage.get()))
}

/// A system notification that the recording is still running. Failures are only logged; the
/// recording carries on either way.
fn remind_still_recording(app: &AppHandle, minutes: u64) {
    info(&format!("reminder: still recording after {minutes} min"));
    let shown = app
        .notification()
        .builder()
        .title("Still recording")
        .body(format!(
            "Meeting Notes has been recording for {minutes} minutes. Press Stop when the meeting is over."
        ))
        .show();
    if let Err(e) = shown {
        warn(&format!("could not show the reminder: {e}"));
    }
}

/// Adds a session to the notes queue and logs it by folder name.
fn enqueue(state: &RunState, session: PathBuf, why: &str) {
    let label = card::session_label(&session);
    let added = state.queue.push(session);
    info(&format!(
        "queue: push session={label} why={why} added={added}"
    ));
}

/// A new recording starts: the old problem and the green check are acknowledged.
fn on_recording_started(app: &AppHandle, state: &RunState) {
    app.state::<CardState>().set_record_failed(None);
    let stale = matches!(
        cards::current_topic(app),
        Some(Topic::RecordFailed | Topic::NotesReady | Topic::SoundTestOk | Topic::StartRefused)
    );
    if stale {
        cards::dismiss(app);
    }
    if state.queue.acknowledge_ready() > 0 {
        emit_queue(app, state);
    }
}

/// Starts a recording on a background thread. Refused only if an update is running or a
/// recording is already active; notes being written for earlier meetings do not matter. When
/// the recording stops, its session goes to the back of the notes queue and the slot is free
/// at once.
fn start_recording(app: &AppHandle, state: &Arc<RunState>) -> Result<(), StartRefusal> {
    begin_recording(app, state, Mode::Meeting)
}

fn begin_recording(app: &AppHandle, state: &Arc<RunState>, mode: Mode) -> Result<(), StartRefusal> {
    if app.state::<UpdateOffer>().is_updating() {
        return Err(StartRefusal::Updating);
    }
    let guard = state.begin().map_err(|_| StartRefusal::Busy)?;
    info(&format!("record: start requested ({mode:?})"));
    match mode {
        Mode::Meeting => on_recording_started(app, state),
        Mode::SoundTest => cards::show_now(app, card::sound_test_running(SOUND_TEST_SECS)),
    }
    let app = app.clone();
    let state = Arc::clone(state);
    thread::Builder::new()
        .name("mn-run".into())
        .spawn(move || run_recording(&app, &state, guard, mode))
        .map(|_| ())
        .map_err(|err| {
            error(&format!("could not start the recording thread: {err}"));
            StartRefusal::Failed
        })
}

/// The body of the recording thread: record, free the slot, then show how it went.
fn run_recording(app: &AppHandle, state: &Arc<RunState>, guard: runner::RunGuard, mode: Mode) {
    let test_dir = (mode == Mode::SoundTest).then(sound_test_dir);
    let result = catch_unwind(AssertUnwindSafe(|| {
        record_session(app, state, test_dir.as_deref())
    }))
    .unwrap_or_else(|_| Err(anyhow!("the recording thread panicked")));
    state.set_recording(false);
    if let Some(dir) = &test_dir {
        remove_sound_test_dir(dir);
    }
    // Free the slot before the outcome is shown, so "Start again" never races the guard.
    drop(guard);
    match (result, mode) {
        (Ok(session), Mode::Meeting) => {
            enqueue(state, session, "recorded");
            emit_queue(app, state);
        }
        (Ok(_), Mode::SoundTest) => {
            info("record: sound test heard sound");
            cards::show_now(app, card::sound_test_ok());
        }
        (Err(err), _) => record_failed(app, &err, mode),
    }
}

/// A recording failed: log the technical reason, tell the window, show the card, notify.
fn record_failed(app: &AppHandle, err: &anyhow::Error, mode: Mode) {
    let (kind, message, hint) = mn_core::classify(err, Stage::Record);
    error(&format!(
        "record: failed kind={} stage=record detail={}",
        kind.as_str(),
        applog::failure_detail(err)
    ));
    emit_status(
        app,
        Status::Error {
            message: message.to_string(),
            kind,
            stage: Stage::Record,
            hint: hint.to_string(),
        },
    );
    app.state::<CardState>().set_record_failed(Some(kind));
    cards::show_now(app, card::recording_failed(kind));
    // The PM is watching a test; a real recording may have failed with the widget hidden.
    if mode == Mode::Meeting {
        cards::notify(app, &card::notice_recording_failed(kind));
    }
}

/// A session's notes finished or failed: log it, notify, and show the card.
fn session_finished(app: &AppHandle, state: &RunState, outcome: &queue::Outcome) {
    let label = card::session_label(&outcome.session);
    match outcome.problem {
        None => {
            info(&format!("queue: finish session={label} ok"));
            cards::notify(app, &card::notice_notes_ready(&label));
            cards::show(app, card::notes_ready(&state.queue.facts().ready));
        }
        Some(problem) => {
            warn(&format!(
                "queue: finish session={label} failed kind={} stage={}",
                problem.kind.as_str(),
                problem.stage.as_str()
            ));
            cards::notify(app, &card::notice_notes_failed(problem.kind));
            cards::show(app, card::notes_failed(&state.queue.facts().failed));
        }
    }
}

/// The one thread that writes the notes: it takes queued sessions in order, one at a time. A
/// failed session is kept for "Try again" and the next one still runs.
fn spawn_queue_worker(app: AppHandle, state: Arc<RunState>) {
    let built = thread::Builder::new()
        .name("mn-queue".into())
        .spawn(move || loop {
            state.queue.wait_for_work();
            let outcome = state.queue.work_one(
                |session| process_session(&app, &state, session),
                || emit_queue(&app, &state),
            );
            if let Some(outcome) = outcome {
                session_finished(&app, &state, &outcome);
            }
        });
    if let Err(e) = built {
        error(&format!("could not start the notes queue: {e}"));
    }
}

/// Queues the sessions an earlier run left unfinished (the app was closed, or the machine shut
/// down, while the notes were being written).
fn recover_unfinished(state: &RunState) {
    let cfg = match Config::load(None) {
        Ok(cfg) => cfg,
        Err(e) => {
            warn(&format!(
                "restart recovery: config: {}",
                applog::failure_detail(&e)
            ));
            return;
        }
    };
    // Nothing is recording at launch, so there is no session to skip.
    let sessions = queue::unfinished_sessions(&cfg.notes_dir, None);
    if !sessions.is_empty() {
        info(&format!(
            "restart recovery: {} unfinished session(s)",
            sessions.len()
        ));
    }
    for session in sessions {
        enqueue(state, session, "restart");
    }
}

#[tauri::command]
fn start(app: AppHandle, state: Shared<'_>) -> Result<(), String> {
    start_recording(&app, &state).map_err(|refusal| {
        cards::start_refused(&app, refusal, false);
        refusal.message().to_string()
    })
}

#[tauri::command]
fn stop(state: Shared<'_>) -> Result<(), String> {
    info("record: stop requested");
    state.request_stop()
}

/// "Try again" for failed notes: queues the failed sessions once more. A failed recording is
/// not retried here; "Start again" starts a new one.
pub(crate) fn retry_notes(app: &AppHandle) {
    if app.state::<UpdateOffer>().is_updating() {
        cards::show_now(app, card::updating());
        return;
    }
    let state = Arc::clone(&*app.state::<Arc<RunState>>());
    let requeued = state.queue.retry_failed();
    info(&format!(
        "queue: retry requested, {requeued} session(s) queued again"
    ));
    if cards::current_topic(app) == Some(Topic::NotesFailed) {
        cards::dismiss(app);
    }
    emit_queue(app, &state);
}

#[tauri::command]
fn retry(app: AppHandle) {
    retry_notes(&app);
}

/// A card button starts a recording (a new one, or a sound test). A refusal is shown on the card.
pub(crate) fn start_by_hand(app: &AppHandle, mode: Mode) {
    let state = Arc::clone(&*app.state::<Arc<RunState>>());
    if let Err(refusal) = begin_recording(app, &state, mode) {
        cards::start_refused(app, refusal, false);
    }
}

/// The queue as it is now, for a window that loaded after the last `mn-queue` event.
#[tauri::command]
fn queue_status(state: Shared<'_>) -> queue::QueueSnapshot {
    state.queue.snapshot()
}

/// The card that is open now, for a window that loaded after the last `mn-card` event.
#[tauri::command]
fn current_card(app: AppHandle) -> cards::CardView {
    cards::current(&app)
}

/// A click on a card button. The name is one of `card::Action`.
#[tauri::command]
fn card_action(app: AppHandle, action: String) {
    match Action::parse(&action) {
        Some(action) => cards::on_action(&app, action),
        None => warn(&format!("card: unknown action {action:?}")),
    }
}

/// A click on the column's status mark: explain what is going on.
#[tauri::command]
fn show_status_card(app: AppHandle) {
    cards::show_status(&app);
}

fn try_open_meetings(app: &AppHandle) -> Result<(), MeetingsProblem> {
    let cfg = Config::load(None).map_err(|err| {
        warn(&format!(
            "open My Meetings: config: {}",
            applog::failure_detail(&err)
        ));
        MeetingsProblem::NotSetUp
    })?;
    let url = meetings::meetings_url(cfg.artifact_url.as_deref(), dirs::config_dir().as_deref())
        .map_err(|err| {
            warn(&format!("open My Meetings: {err}"));
            MeetingsProblem::LinkMissing
        })?;
    app.opener().open_url(url, None::<&str>).map_err(|err| {
        warn(&format!("open My Meetings: browser: {err}"));
        MeetingsProblem::Browser
    })
}

/// Opens My Meetings. On success the green check is acknowledged; on failure the card says why.
pub(crate) fn open_meetings_now(app: &AppHandle) -> Result<(), String> {
    match try_open_meetings(app) {
        Ok(()) => {
            let state = Arc::clone(&*app.state::<Arc<RunState>>());
            if state.queue.acknowledge_ready() > 0 {
                emit_queue(app, &state);
            }
            if cards::current_topic(app) == Some(Topic::NotesReady) {
                cards::dismiss(app);
            }
            Ok(())
        }
        Err(problem) => {
            cards::show_now(app, card::meetings_problem(problem));
            Err("Could not open My Meetings.".to_string())
        }
    }
}

#[tauri::command]
fn open_meetings(app: AppHandle) -> Result<(), String> {
    open_meetings_now(&app)
}

struct NotchState {
    prefs: Mutex<WidgetPrefs>,
    prefs_path: Option<PathBuf>,
    anchor: Mutex<Option<Anchor>>,
    /// Whether the window is the folded tab (true) or the unfolded column. Kept here so the
    /// placement maths never depends on reading a size back from a window that is still resizing.
    folded: AtomicBool,
    /// The message card is open, so the window is wider (and taller) than the column.
    card: AtomicBool,
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
            card: AtomicBool::new(false),
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
                warn(&format!("could not save {}: {e}", path.display()));
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

/// Physical size of the window in its current state: folded tab, column, or column plus card.
fn physical_size(window: &Window, folded: bool, card: bool) -> (i32, i32) {
    let scale = window.scale_factor().unwrap_or(1.0);
    let (w, h) = notch::size_for_card(folded, card);
    ((w * scale).round() as i32, (h * scale).round() as i32)
}

/// Where the window's top-left corner belongs for the current anchor and fold state.
fn target_position(window: &Window, state: &NotchState) -> Option<(i32, i32)> {
    let anchor = state.anchor()?;
    let (w, h) = physical_size(
        window,
        state.folded.load(Ordering::SeqCst),
        state.card.load(Ordering::SeqCst),
    );
    Some(anchor.window_pos(w, h))
}

/// Moves the window to its anchored spot, hanging from the edge.
fn place(window: &Window, state: &NotchState) {
    let Some((x, y)) = target_position(window, state) else {
        return;
    };
    state.mark_own_move();
    if let Err(e) = window.set_position(PhysicalPosition::new(x, y)) {
        warn(&format!("could not position the window: {e}"));
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
    info(&format!(
        "NOTCH {label}size={size} pos={pos} visible={visible}"
    ));
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

/// Resizes between the folded tab, the unfolded column and the column with the message card
/// beside it, keeping the same edge and the same vertical centre.
fn apply_size(window: &Window, state: &NotchState, folded: bool) -> Result<(), String> {
    let (width, height) = notch::size_for_card(folded, state.card.load(Ordering::SeqCst));
    state.folded.store(folded, Ordering::SeqCst);
    lock_size(window, LogicalSize::new(width, height))
        .map_err(|e| format!("Could not resize the window: {e}"))?;
    if !state.dragging.load(Ordering::SeqCst) && !state.user_hidden.load(Ordering::SeqCst) {
        place(window, state);
    }
    if !state.user_hidden.load(Ordering::SeqCst) && !window.is_visible().unwrap_or(true) {
        let _ = window.show();
    }
    report(window, "");
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

#[tauri::command]
fn set_size(window: Window, state: State<'_, NotchState>, folded: bool) -> Result<(), String> {
    apply_size(&window, &state, folded)
}

/// Makes room for the message card beside the column (the window grows toward the screen
/// centre), or takes it back. The column does not move on screen.
fn set_card_open(app: &AppHandle, open: bool) {
    let state = app.state::<NotchState>();
    if state.card.swap(open, Ordering::SeqCst) == open {
        return;
    }
    let Some(webview) = app.get_webview_window(WINDOW_LABEL) else {
        return;
    };
    let window = webview.as_ref().window();
    let folded = state.folded.load(Ordering::SeqCst);
    if let Err(e) = apply_size(&window, &state, folded) {
        warn(&e);
    }
}

/// Physical pixels from the top of the card window to the top of the column inside it.
fn column_offset_px(window: &Window, state: &NotchState) -> i32 {
    let Some(anchor) = state.anchor() else {
        return 0;
    };
    let (_, window_h) = physical_size(window, false, true);
    let (_, column_h) = physical_size(window, false, false);
    notch::column_offset(&anchor, window_h, column_h)
}

/// Logical pixels from the window's top to the column's top while the card is open, else 0. The
/// window shows the column at this offset so it stays where it was on screen.
fn column_top(app: &AppHandle) -> f64 {
    let state = app.state::<NotchState>();
    if !state.card.load(Ordering::SeqCst) || state.folded.load(Ordering::SeqCst) {
        return 0.0;
    }
    let Some(webview) = app.get_webview_window(WINDOW_LABEL) else {
        return 0.0;
    };
    let window = webview.as_ref().window();
    let scale = window.scale_factor().unwrap_or(1.0);
    f64::from(column_offset_px(&window, &state)) / scale
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
        warn(&format!("could not send the edge to the window: {e}"));
    }
}

/// Puts the window at the saved (or default) spot. Runs once, before the window is shown.
fn init_position(window: &Window, state: &NotchState) {
    let Some((rects, primary)) = monitors(window) else {
        warn("no monitor information; leaving the window where the system put it");
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
    let folded = state.folded.load(Ordering::SeqCst);
    let card = state.card.load(Ordering::SeqCst) && !folded;
    let (w, h) = physical_size(window, folded, card);
    // With the card open the window is wider than the column, so the drop point is the
    // column's centre, not the window's.
    let (drop_x, drop_y) = if card {
        let edge = state.anchor().map_or(Edge::Right, |a| a.edge);
        let column = physical_size(window, false, false);
        let offset = column_offset_px(window, &state);
        notch::column_center(edge, (pos.x, pos.y), (w, h), column, offset)
    } else {
        (pos.x + w / 2, pos.y + h / 2)
    };
    let anchor = notch::drop_anchor(drop_x, drop_y, &rects, primary);
    state.set_anchor(anchor);
    state.dragging.store(false, Ordering::SeqCst);
    place(window, &state);
    emit_edge(window, anchor.edge);
    // The column may sit at another height in the window now.
    cards::resend(window.app_handle());
    let saved = anchor.saved(drop_x);
    state.update_prefs(|p| p.pos = Some(saved));
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
    info(&format!("show: {}", describe_window(&window)));
    // Showing is asynchronous on X11: check again once it has happened.
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(200));
        ensure_placed(&window);
        info(&format!("show settled: {}", describe_window(&window)));
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
    info(&format!("hide: {}", describe_window(&window)));
}

/// Decides by our own flag, not by `is_visible()`: on X11 that answer lags behind a show or
/// hide, so a quick second press used to read the old state and do the wrong thing.
fn toggle_window(app: &AppHandle) {
    let hidden = app.state::<NotchState>().user_hidden.load(Ordering::SeqCst);
    info(&format!("Ctrl+Alt+M: hidden={hidden}"));
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
    // The widget may be hidden when a hotkey is used, so a refusal is also a notification.
    if let Err(refusal) = start_recording(app, &state) {
        cards::start_refused(app, refusal, true);
    }
}

/// "Ctrl+Alt+M" for the PM's keyboard ("Control+Option+M" on a Mac).
fn shortcut_name(key: char) -> String {
    let (control, alt) = if cfg!(target_os = "macos") {
        ("Control", "Option")
    } else {
        ("Ctrl", "Alt")
    };
    format!("{control}+{alt}+{key}")
}

/// Ctrl+Alt+M (Control+Option+M on macOS) shows/hides the tab; Ctrl+Alt+R starts/stops a
/// recording. The plugin is added here, not on the builder, because its own setup fails when
/// there is no X11 connection (Wayland without XWayland), and a failed plugin setup would stop
/// the whole app. A failure is logged and shown on the card; the widget's buttons still work.
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
    let names = [shortcut_name('M'), shortcut_name('R')];
    if let Err(e) = app.plugin(plugin) {
        warn(&format!("the hotkeys are off: {e}"));
        tell_hotkeys_off(app, &[&names[0], &names[1]]);
        return;
    }
    let mut off = Vec::new();
    for (shortcut, name) in [(toggle, &names[0]), (record, &names[1])] {
        if let Err(e) = app.global_shortcut().register(shortcut) {
            warn(&format!("the {name} hotkey is off: {e}"));
            off.push(name.as_str());
        }
    }
    tell_hotkeys_off(app, &off);
}

/// Shows which keyboard shortcuts could not be turned on, if any.
fn tell_hotkeys_off(app: &AppHandle, names: &[&str]) {
    if !names.is_empty() {
        cards::show(app, card::hotkeys_off(names));
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
        Err(e) => warn(&format!("could not turn on start at login: {e}")),
    }
}

/// Linux: the autostart plugin writes `Exec=` with no quotes, so a path with spaces never
/// starts. Quote it, and point an AppImage at `$APPIMAGE` rather than the temporary mount.
/// No-op on Windows and macOS.
#[cfg(target_os = "linux")]
fn fix_linux_autostart(app: &AppHandle) {
    let Some(home) = dirs::home_dir() else {
        warn("could not fix start at login: no home directory");
        return;
    };
    let file = desktop_entry::autostart_desktop_path(&home, &app.package_info().name);
    let current = match std::env::current_exe() {
        Ok(path) => path.display().to_string(),
        Err(err) => {
            warn(&format!("could not fix start at login: {err}"));
            return;
        }
    };
    let appimage = std::env::var("APPIMAGE").ok();
    let launch = desktop_entry::launch_target(appimage.as_deref(), &current);
    if let Err(err) = desktop_entry::rewrite_autostart_file(&file, &launch) {
        warn(&format!("could not fix start at login: {err}"));
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
        warn(&format!("could not change start at login: {e}"));
    } else if turning_on {
        fix_linux_autostart(app);
    }
    // Show what is really set, whether or not the change worked.
    let _ = item.set_checked(autostart_enabled(app));
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let mut login = None;
    let mut items: Vec<Box<dyn IsMenuItem<tauri::Wry>>> = Vec::new();
    for entry in menu::TRAY {
        if entry.checkable {
            let item = CheckMenuItem::with_id(
                app,
                entry.id,
                entry.label,
                true,
                autostart_enabled(app),
                None::<&str>,
            )?;
            login = Some(item.clone());
            items.push(Box::new(item));
        } else {
            items.push(Box::new(MenuItem::with_id(
                app,
                entry.id,
                entry.label,
                true,
                None::<&str>,
            )?));
        }
    }
    let refs: Vec<&dyn IsMenuItem<tauri::Wry>> = items.iter().map(|item| item.as_ref()).collect();
    let menu = Menu::with_items(app, &refs)?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Meeting Notes")
        .menu(&menu)
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "hide" => hide_window(app),
            "autostart" => {
                if let Some(item) = &login {
                    toggle_autostart(app, item);
                }
            }
            "update" => begin_update(app, true),
            "open_logs" => cards::open_log_folder(app),
            "report" => {
                // The card shows where the report is, so the widget must be visible.
                show_window(app);
                cards::start_report(app);
            }
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

impl UpdateOffer {
    fn is_updating(&self) -> bool {
        self.updating.load(Ordering::SeqCst)
    }
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

/// Tells the window about an update step, and puts the words of failures, refusals and
/// confirmations on the message card instead of in a tooltip.
fn emit_update(app: &AppHandle, payload: &UpdatePayload) {
    if let Err(err) = app.emit(UPDATE_EVENT, payload) {
        warn(&format!("could not send the update to the window: {err}"));
    }
    if matches!(payload.phase.as_str(), "failed" | "timeout") {
        warn(&format!(
            "update {}: {}",
            payload.phase,
            mn_core::logging::scrub(&payload.message)
        ));
    }
    if let Some(card) = card::update_card(&payload.phase, &payload.message) {
        cards::show_now(app, card);
    }
}

fn notify(app: &AppHandle, body: &str) {
    cards::notify(
        app,
        &card::Notice {
            title: "Meeting Notes".to_string(),
            body: body.to_string(),
        },
    );
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
                    warn(&format!("could not rename the update tray item: {err}"));
                }
            } else if let Some(menu) = menu {
                match MenuItem::with_id(app, "update", &label, true, None::<&str>) {
                    Ok(item) => {
                        // After Start at login, before the help items and Quit.
                        if let Err(err) = menu.insert(&item, menu::update_insert_index()) {
                            warn(&format!("could not add the update tray item: {err}"));
                        } else {
                            *slot = Some(item);
                        }
                    }
                    Err(err) => warn(&format!("could not create the update tray item: {err}")),
                }
            } else {
                warn("update check: the tray menu is not ready");
            }
        }
        Err(_) => warn("update check: the tray state is locked"),
    }
    info(&format!("update available {version}"));
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
                    Err(err) => warn(&format!("update check: {err}")),
                }
                thread::sleep(update::CHECK_EVERY);
            }
        });
    if let Err(err) = built {
        warn(&format!("could not start the update check: {err}"));
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
        info("update: no updater script, copying the repair sentence");
        copy_repair(app, from_tray);
        return;
    }
    let Some(status) = update_status_path() else {
        warn("update: no config directory, copying the repair sentence");
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
            warn(&format!("update: {err}"));
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
        warn(&format!("update: {err}"));
        tell_update(
            app,
            from_tray,
            "failed",
            "Update failed. The updater could not be started.",
        );
        return;
    }
    offer.updating.store(true, Ordering::SeqCst);
    info(&format!("update: started {}", spec.program));
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
        warn(&format!("could not copy the update prompt: {err}"));
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
                    warn("update: no finished status after 15 minutes");
                    // Start must not stay disabled for good: clear the wait and say so.
                    finish_update_wait(&worker, "timeout", "The update is taking too long.");
                    break;
                }
                let status = match self_update::read_status(&path) {
                    Ok(Some(status)) => status,
                    Ok(None) => continue,
                    Err(err) => {
                        warn(&format!("update status: {err}"));
                        continue;
                    }
                };
                match self_update::watch_step(&status, announce_done) {
                    self_update::PollOutcome::Wait => {}
                    self_update::PollOutcome::UpToDate => {
                        if let Err(err) = self_update::mark_seen(&path) {
                            warn(&format!("update status: {err}"));
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
                            warn(&format!("update status: {err}"));
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
        warn(&format!("could not watch the update: {err}"));
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
            warn(&format!("update status: {err}"));
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
                warn(&format!("update status: {err}"));
            }
            quiet_payload()
        }
        self_update::StartNotice::Failed { message } => {
            warn(&format!(
                "update failed (found at start): {}",
                mn_core::logging::scrub(&message)
            ));
            if let Some(card) = card::update_card("failed", &message) {
                cards::show_now(&app, card);
            }
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
fn copy_help_now(app: &AppHandle) -> Result<(), String> {
    let prompt = repair_prompt();
    app.clipboard()
        .write_text(&prompt)
        .map_err(|err| format!("Could not copy: {err}"))?;
    if let Some(path) = update_status_path() {
        if let Err(err) = self_update::mark_seen(&path) {
            warn(&format!("update status: {err}"));
        }
    }
    emit_update(
        app,
        &update_payload("copied", "", &prompt, self_update::COPY_HELP),
    );
    Ok(())
}

#[tauri::command]
fn copy_update_help(app: AppHandle) -> Result<(), String> {
    copy_help_now(&app)
}

/// The card's "Get help" button. A failed copy is said on the card.
pub(crate) fn get_update_help(app: &AppHandle) {
    if let Err(err) = copy_help_now(app) {
        warn(&format!("update help: {err}"));
        if let Some(card) = card::update_card("refused", "Could not copy the update steps.") {
            cards::show_now(app, card);
        }
    }
}

/// The card's "Dismiss" on a failed update: later launches stay quiet and the widget goes back to
/// its normal look.
pub(crate) fn dismiss_update_failure(app: &AppHandle) {
    if let Some(path) = update_status_path() {
        if let Err(err) = self_update::mark_seen(&path) {
            warn(&format!("update status: {err}"));
        }
    }
    emit_update(app, &quiet_payload());
}

/// Fallback when the webview clipboard refuses the write. The button click is the gesture.
#[tauri::command]
fn copy_update_prompt(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|err| format!("Could not copy: {err}"))
}

pub fn run() {
    mn_core::logging::header("widget");
    applog::install_panic_hook();
    tauri::Builder::default()
        // First plugin: a second launch exits and this callback runs in the copy that is
        // already running.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            info("second launch: showing the widget");
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
        .manage(CardState::default())
        .invoke_handler(tauri::generate_handler![
            start,
            stop,
            retry,
            queue_status,
            current_card,
            card_action,
            show_status_card,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sound_test_folder_is_new_every_time_and_sits_in_the_temp_folder() {
        let first = sound_test_dir();
        assert!(is_sound_test_dir(&first, &std::env::temp_dir()));
        assert_eq!(first.parent(), Some(std::env::temp_dir().as_path()));
        std::thread::sleep(Duration::from_millis(2));
        assert_ne!(first, sound_test_dir());
    }

    #[test]
    fn only_a_sound_test_folder_may_be_removed() {
        let temp = Path::new("/tmp");
        assert!(is_sound_test_dir(
            Path::new("/tmp/meeting-notes-sound-test-12-3456"),
            temp
        ));
        // Real notes, other temp files, the temp folder itself, and nested paths are all refused.
        for refused in [
            "/home/someone/Meeting Notes/2026-10-24_1405",
            "/tmp/other-folder",
            "/tmp",
            "/",
            "/tmp/meeting-notes-sound-test-12-3456/inner",
            "/var/tmp/meeting-notes-sound-test-12-3456",
        ] {
            assert!(!is_sound_test_dir(Path::new(refused), temp), "{refused}");
        }
    }

    #[test]
    fn removing_a_test_folder_removes_it_and_leaves_other_folders_alone() {
        let temp = std::env::temp_dir();
        let test = sound_test_dir();
        std::fs::create_dir_all(test.join("session")).unwrap();
        let other = tempfile::tempdir().unwrap();
        remove_sound_test_dir(&test);
        assert!(!test.exists());
        // A folder that is not a test folder is refused (and logged as a warning).
        assert!(!is_sound_test_dir(other.path(), &temp));
        assert!(other.path().exists());
    }

    #[test]
    fn shortcut_names_follow_the_keys_on_the_keyboard() {
        let name = shortcut_name('R');
        if cfg!(target_os = "macos") {
            assert_eq!(name, "Control+Option+R");
        } else {
            assert_eq!(name, "Ctrl+Alt+R");
        }
    }
}
