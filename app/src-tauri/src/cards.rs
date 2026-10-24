//! Shows the message card: keeps the one card that is on screen, makes room for it beside the
//! column, sends system notifications, and runs what the card buttons do. The words and the
//! rules live in `card.rs`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use mn_core::ErrorKind;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

use crate::applog::{error, info, warn};
use crate::card::{self, Action, Card, Notice, Slot, StartRefusal, Topic};
use crate::runner::RunState;
use crate::{Mode, UpdateOffer};

/// The card on screen. Payload is `CardView`; `card: null` means it was closed.
pub const CARD_EVENT: &str = "mn-card";

/// The card as the window draws it, plus where the column sits inside the taller window.
#[derive(Debug, Clone, Serialize)]
pub struct CardView {
    pub card: Option<Card>,
    /// Logical pixels from the window's top to the column's top, so the column does not move on
    /// screen when the window grows around it.
    pub column_top: f64,
}

#[derive(Default)]
pub struct CardState {
    slot: Mutex<Slot>,
    /// The last recording failed and the PM has not dismissed it or started again.
    record_failed: Mutex<Option<ErrorKind>>,
    /// The report zip made last, for "Show the file".
    report_file: Mutex<Option<PathBuf>>,
}

impl CardState {
    pub fn record_failed(&self) -> Option<ErrorKind> {
        self.record_failed.lock().ok().and_then(|kind| *kind)
    }

    pub fn set_record_failed(&self, kind: Option<ErrorKind>) {
        if let Ok(mut slot) = self.record_failed.lock() {
            *slot = kind;
        }
    }
}

/// Shows `card` unless a more important one is on screen.
pub fn show(app: &AppHandle, card: Card) {
    present(app, card, false);
}

/// Shows `card` whatever is on screen. For a card the PM just asked for.
pub fn show_now(app: &AppHandle, card: Card) {
    present(app, card, true);
}

fn present(app: &AppHandle, card: Card, force: bool) {
    let state = app.state::<CardState>();
    let shown = state
        .slot
        .lock()
        .ok()
        .and_then(|mut slot| slot.offer(card.clone(), force));
    let Some(seq) = shown else {
        info(&format!("card kept back: {:?}", card.topic));
        return;
    };
    info(&format!("card shown: {:?}", card.topic));
    // The window grows before the card is drawn, and shrinks after it is gone.
    crate::set_card_open(app, true);
    send(app, Some(card.clone()));
    if let Some(ms) = card.auto_hide_ms {
        hide_later(app.clone(), seq, ms);
    }
}

fn send(app: &AppHandle, card: Option<Card>) {
    let view = CardView {
        card,
        column_top: crate::column_top(app),
    };
    if let Err(err) = app.emit(CARD_EVENT, view) {
        warn(&format!("could not send the card to the window: {err}"));
    }
}

fn close(app: &AppHandle) {
    send(app, None);
    crate::set_card_open(app, false);
}

fn hide_later(app: AppHandle, seq: u64, ms: u64) {
    let built = thread::Builder::new()
        .name("mn-card-timer".into())
        .spawn(move || {
            thread::sleep(Duration::from_millis(ms));
            let cleared = app
                .state::<CardState>()
                .slot
                .lock()
                .is_ok_and(|mut slot| slot.clear_if(seq));
            if cleared {
                close(&app);
            }
        });
    if let Err(err) = built {
        warn(&format!("could not start the card timer: {err}"));
    }
}

/// Closes the card, if one is showing. Returns what it was about.
pub fn dismiss(app: &AppHandle) -> Option<Topic> {
    let closed = app
        .state::<CardState>()
        .slot
        .lock()
        .ok()
        .and_then(|mut slot| slot.clear());
    let topic = closed.map(|card| card.topic);
    if topic.is_some() {
        close(app);
    }
    topic
}

/// What the card shows now, for a window that loaded after the last `mn-card` event.
pub fn current(app: &AppHandle) -> CardView {
    let card = app
        .state::<CardState>()
        .slot
        .lock()
        .ok()
        .and_then(|slot| slot.current().cloned());
    CardView {
        card,
        column_top: crate::column_top(app),
    }
}

/// The topic on screen, if any.
pub fn current_topic(app: &AppHandle) -> Option<Topic> {
    current(app).card.map(|card| card.topic)
}

/// Sends the card again. After a drop the column may sit at a different place in the window.
pub fn resend(app: &AppHandle) {
    if let Some(card) = current(app).card {
        send(app, Some(card));
    }
}

/// A system notification. A failure is only logged.
pub fn notify(app: &AppHandle, notice: &Notice) {
    let shown = app
        .notification()
        .builder()
        .title(notice.title.clone())
        .body(notice.body.clone())
        .show();
    if let Err(err) = shown {
        warn(&format!("could not show a notification: {err}"));
    }
}

/// Start did nothing: say why on the card, and (for a hotkey, where the widget may be hidden)
/// in a notification too.
pub fn start_refused(app: &AppHandle, refusal: StartRefusal, also_notify: bool) {
    warn(&format!("start refused: {refusal:?}"));
    show_now(app, refusal.card());
    if also_notify {
        notify(
            app,
            &Notice {
                title: "Could not start the recording".to_string(),
                body: refusal.message().to_string(),
            },
        );
    }
}

/// What a click on a card button does.
pub fn on_action(app: &AppHandle, action: Action) {
    info(&format!("card action: {action:?}"));
    match action {
        Action::Dismiss => dismiss_current(app),
        Action::Retry => crate::retry_notes(app),
        Action::StartAgain => crate::start_by_hand(app, Mode::Meeting),
        Action::ShortTest => crate::start_by_hand(app, Mode::SoundTest),
        Action::Report => start_report(app),
        Action::OpenMeetings => {
            // A failure shows its own card.
            let _ = crate::open_meetings_now(app);
        }
        Action::ShowFile => show_report_file(app),
        Action::OpenLogs => open_log_folder(app),
        Action::GetHelp => crate::get_update_help(app),
    }
}

/// Dismiss also clears what the card was about, so the column stops showing it.
fn dismiss_current(app: &AppHandle) {
    match dismiss(app) {
        Some(Topic::RecordFailed) => app.state::<CardState>().set_record_failed(None),
        Some(Topic::UpdateFailed) => crate::dismiss_update_failure(app),
        _ => {}
    }
}

/// What the column's status mark shows when clicked.
pub fn show_status(app: &AppHandle) {
    let run = app.state::<std::sync::Arc<RunState>>();
    let mut facts = run.queue.facts();
    facts.updating = app.state::<UpdateOffer>().is_updating();
    facts.recording_failed = app.state::<CardState>().record_failed();
    if let Some(card) = card::status_card(&facts) {
        show_now(app, card);
    }
}

/// The working card follows the stage while it is open. When the session is over, the outcome
/// card replaces it, so nothing happens here.
pub fn refresh_working(app: &AppHandle, state: &RunState) {
    if current_topic(app) != Some(Topic::Working) {
        return;
    }
    let facts = state.queue.facts();
    if let Some(stage) = facts.processing {
        show(app, card::working(&stage, facts.waiting));
    }
}

/// "Report a problem": builds the zip off the UI thread, then says where it is.
pub fn start_report(app: &AppHandle) {
    info("report: requested");
    show_now(app, card::report_working());
    let worker = app.clone();
    let built = thread::Builder::new()
        .name("mn-report".into())
        .spawn(move || {
            let made = std::panic::catch_unwind(|| mn_core::create_report(None));
            match made {
                Ok(Ok(path)) => report_made(&worker, path),
                Ok(Err(err)) => report_failed(&worker, &crate::applog::failure_detail(&err)),
                Err(_) => report_failed(&worker, "the report thread panicked"),
            }
        });
    if let Err(err) = built {
        report_failed(app, &err.to_string());
    }
}

fn report_made(app: &AppHandle, path: PathBuf) {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let on_desktop = dirs::desktop_dir().is_some_and(|desktop| path.parent() == Some(&desktop));
    info(&format!("report: created {name} on_desktop={on_desktop}"));
    if let Ok(mut slot) = app.state::<CardState>().report_file.lock() {
        *slot = Some(path);
    }
    show_now(app, card::report_ready(&name, on_desktop));
}

fn report_failed(app: &AppHandle, detail: &str) {
    error(&format!("report: failed: {detail}"));
    show_now(app, card::report_failed());
}

/// Opens the folder that holds the report, with the file selected where the system can.
fn show_report_file(app: &AppHandle) {
    let file = app
        .state::<CardState>()
        .report_file
        .lock()
        .ok()
        .and_then(|slot| slot.clone());
    let Some(file) = file else {
        warn("report: show the file asked, but no report was made");
        return;
    };
    if let Err(err) = app.opener().reveal_item_in_dir(&file) {
        warn(&format!("report: could not reveal the file: {err}"));
        match file.parent() {
            Some(folder) => open_folder(app, folder),
            None => warn("report: the file has no folder"),
        }
    }
}

/// "Open log folder" (card and tray).
pub fn open_log_folder(app: &AppHandle) {
    let dir = mn_core::logging::log_dir();
    if let Err(err) = std::fs::create_dir_all(&dir) {
        warn(&format!("could not create the log folder: {err}"));
    }
    open_folder(app, &dir);
}

fn open_folder(app: &AppHandle, folder: &std::path::Path) {
    let opened = app
        .opener()
        .open_path(folder.to_string_lossy().into_owned(), None::<&str>);
    if let Err(err) = opened {
        warn(&format!("could not open a folder: {err}"));
        let shown = card::home_relative(folder, dirs::home_dir().as_deref());
        show_now(app, card::folder_failed(&shown));
    }
}
