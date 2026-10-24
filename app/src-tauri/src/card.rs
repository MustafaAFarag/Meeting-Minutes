//! The message card: what it says in each state, and which card wins when two want the screen.
//! Pure text and rules. Nothing here touches Tauri, so all of it is tested.

use std::path::Path;

use mn_core::ErrorKind;
use serde::Serialize;

/// A card that is only good news hides itself after this long.
pub const AUTO_HIDE_MS: u64 = 20_000;
/// How many failed meetings the card lists before "and N more".
const SHOWN_FAILURES: usize = 2;

/// What the card is about. The window uses it to decide what Dismiss also clears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    RecordFailed,
    NotesFailed,
    NotesReady,
    Working,
    Updating,
    UpdateFailed,
    UpdateNotice,
    UpdateTimeout,
    MeetingsLink,
    HotkeysOff,
    StartRefused,
    SoundTestRunning,
    SoundTestOk,
    ReportWorking,
    ReportReady,
    ReportFailed,
}

/// What a card button does. The window sends the snake_case name back to `card_action`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Dismiss,
    Retry,
    StartAgain,
    ShortTest,
    Report,
    OpenMeetings,
    ShowFile,
    OpenLogs,
    GetHelp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Button {
    pub action: Action,
    pub label: String,
    pub primary: bool,
}

/// One card, as the window draws it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Card {
    pub topic: Topic,
    /// ARIA role: "alertdialog" for errors, "status" for everything else.
    pub role: &'static str,
    /// "error", "ok" or "info": the colour of the card.
    pub tone: &'static str,
    pub title: String,
    pub lines: Vec<String>,
    pub buttons: Vec<Button>,
    pub auto_hide_ms: Option<u64>,
}

/// One session whose notes failed. `name` is the session folder name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedItem {
    pub name: String,
    pub kind: ErrorKind,
}

/// Everything the card can be asked to explain.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub updating: bool,
    pub recording_failed: Option<ErrorKind>,
    pub failed: Vec<FailedItem>,
    /// The stage of the session being written, in words.
    pub processing: Option<String>,
    pub waiting: usize,
    /// Session folder names whose notes are on My Meetings and not opened yet.
    pub ready: Vec<String>,
}

/// What went wrong when "Open My Meetings" was clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingsProblem {
    LinkMissing,
    NotSetUp,
    Browser,
}

/// A system notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// Holds the card on screen and decides who may replace it.
#[derive(Debug, Default)]
pub struct Slot {
    card: Option<Card>,
    seq: u64,
}

impl Topic {
    /// Errors interrupt a screen reader; everything else is announced politely.
    pub fn role(self) -> &'static str {
        if self.is_error() {
            "alertdialog"
        } else {
            "status"
        }
    }

    pub fn tone(self) -> &'static str {
        match self {
            _ if self.is_error() => "error",
            Self::NotesReady | Self::SoundTestOk | Self::ReportReady => "ok",
            _ => "info",
        }
    }

    fn is_error(self) -> bool {
        matches!(
            self,
            Self::RecordFailed
                | Self::NotesFailed
                | Self::UpdateFailed
                | Self::UpdateTimeout
                | Self::MeetingsLink
                | Self::StartRefused
                | Self::ReportFailed
        )
    }

    /// An error is never pushed off the screen by good news or by progress.
    fn rank(self) -> u8 {
        if self.is_error() {
            3
        } else {
            1
        }
    }
}

impl Action {
    /// The button name the window sends back. Anything unknown is refused.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "dismiss" => Self::Dismiss,
            "retry" => Self::Retry,
            "start_again" => Self::StartAgain,
            "short_test" => Self::ShortTest,
            "report" => Self::Report,
            "open_meetings" => Self::OpenMeetings,
            "show_file" => Self::ShowFile,
            "open_logs" => Self::OpenLogs,
            "get_help" => Self::GetHelp,
            _ => return None,
        })
    }
}

impl Slot {
    /// Puts `card` on screen unless a more important one is there. `force` is for a card the
    /// user just asked for. Returns a number that identifies this showing, for the auto-hide
    /// timer.
    pub fn offer(&mut self, card: Card, force: bool) -> Option<u64> {
        if let Some(current) = &self.card {
            let weaker = card.topic.rank() < current.topic.rank();
            if !force && weaker && card.topic != current.topic {
                return None;
            }
        }
        self.seq += 1;
        self.card = Some(card);
        Some(self.seq)
    }

    /// Closes the card and returns it.
    pub fn clear(&mut self) -> Option<Card> {
        self.card.take()
    }

    /// Closes the card only if it is still the showing numbered `seq`: an old timer must not
    /// close a newer card.
    pub fn clear_if(&mut self, seq: u64) -> bool {
        if self.seq == seq && self.card.is_some() {
            self.card = None;
            return true;
        }
        false
    }

    pub fn current(&self) -> Option<&Card> {
        self.card.as_ref()
    }
}

/// `YYYY-MM-DD_HHMM`, with an optional `-N` for the second meeting of one recording.
fn is_session_name(name: &str) -> bool {
    let b = name.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        b.get(range)
            .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
    };
    let stem = digits(0..4)
        && b.get(4) == Some(&b'-')
        && digits(5..7)
        && b.get(7) == Some(&b'-')
        && digits(8..10)
        && b.get(10) == Some(&b'_')
        && digits(11..15);
    stem && match b.get(15..) {
        Some([]) | None => true,
        Some([b'-', rest @ ..]) => !rest.is_empty() && rest.iter().all(u8::is_ascii_digit),
        Some(_) => false,
    }
}

/// The folder name of a session, which is only a date and time. Any other folder name could hold
/// a client name, so it becomes "session". Safe for the log.
pub fn session_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| is_session_name(name))
        .unwrap_or("session")
        .to_string()
}

/// "14:05" for a session started at 14:05.
pub fn session_time(label: &str) -> Option<String> {
    if !is_session_name(label) {
        return None;
    }
    let hour: u32 = label.get(11..13)?.parse().ok()?;
    let minute: u32 = label.get(13..15)?.parse().ok()?;
    (hour < 24 && minute < 60).then(|| format!("{hour:02}:{minute:02}"))
}

/// "your 14:05 meeting", or "your meeting" when the time is unknown.
fn your_meeting(label: &str) -> String {
    match session_time(label) {
        Some(time) => format!("your {time} meeting"),
        None => "your meeting".to_string(),
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

fn button(action: Action, label: &str, primary: bool) -> Button {
    Button {
        action,
        label: label.to_string(),
        primary,
    }
}

fn report_button() -> Button {
    button(Action::Report, "Report a problem", false)
}

fn dismiss_button() -> Button {
    button(Action::Dismiss, "Dismiss", false)
}

fn card(topic: Topic, title: &str, lines: Vec<String>, buttons: Vec<Button>) -> Card {
    Card {
        topic,
        role: topic.role(),
        tone: topic.tone(),
        title: title.to_string(),
        lines,
        buttons,
        auto_hide_ms: None,
    }
}

fn auto_hiding(mut card: Card) -> Card {
    card.auto_hide_ms = Some(AUTO_HIDE_MS);
    card
}

/// A recording that failed. The button starts a new recording, so it says "Start again", and the
/// hint (written for notes) is reworded to match.
pub fn recording_failed(kind: ErrorKind) -> Card {
    let hint = kind.hint().replace("Try again", "Start again");
    let first = match kind {
        ErrorKind::NoSound | ErrorKind::MicUnavailable => {
            button(Action::ShortTest, "Try a short test", true)
        }
        _ => button(Action::StartAgain, "Start again", true),
    };
    card(
        Topic::RecordFailed,
        kind.message(),
        vec![hint],
        vec![first, report_button(), dismiss_button()],
    )
}

/// Notes that could not be written. Several failed meetings share one card.
pub fn notes_failed(failed: &[FailedItem]) -> Card {
    match failed {
        [] => card(
            Topic::NotesFailed,
            ErrorKind::Unknown.message(),
            vec![ErrorKind::Unknown.hint().to_string()],
            vec![dismiss_button()],
        ),
        [one] => {
            let title = if one.kind == ErrorKind::Publish {
                format!(
                    "{} is not on My Meetings yet.",
                    capitalise(&your_meeting(&one.name))
                )
            } else {
                format!(
                    "Notes for {} could not be written.",
                    your_meeting(&one.name)
                )
            };
            card(
                Topic::NotesFailed,
                &title,
                vec![one.kind.message().to_string(), one.kind.hint().to_string()],
                vec![
                    button(Action::Retry, "Try again", true),
                    report_button(),
                    dismiss_button(),
                ],
            )
        }
        many => {
            let mut lines: Vec<String> = many
                .iter()
                .take(SHOWN_FAILURES)
                .map(|item| {
                    let which = session_time(&item.name)
                        .map_or_else(|| "A meeting".to_string(), |t| format!("{t} meeting"));
                    format!("{which}: {}", item.kind.message())
                })
                .collect();
            if many.len() > SHOWN_FAILURES {
                lines.push(format!("and {} more", many.len() - SHOWN_FAILURES));
            }
            if let Some(newest) = many.last() {
                lines.push(newest.kind.hint().to_string());
            }
            card(
                Topic::NotesFailed,
                &format!("{} meetings need attention", many.len()),
                lines,
                vec![
                    button(Action::Retry, "Try again all", true),
                    report_button(),
                    dismiss_button(),
                ],
            )
        }
    }
}

/// Success: the notes are on My Meetings. Hides itself after 20 seconds.
pub fn notes_ready(ready: &[String]) -> Card {
    let line = match ready {
        [one] => format!("{} is on My Meetings.", capitalise(&your_meeting(one))),
        many => format!("{} meetings are on My Meetings.", many.len()),
    };
    auto_hiding(card(
        Topic::NotesReady,
        "Notes ready",
        vec![line],
        vec![button(Action::OpenMeetings, "Open My Meetings", true)],
    ))
}

/// Loading: the stage in words, the usual time, and how many meetings wait behind this one.
pub fn working(stage: &str, waiting: usize) -> Card {
    let mut lines = vec![stage.to_string(), "Usually 2-5 minutes".to_string()];
    if waiting > 0 {
        let noun = if waiting == 1 { "meeting" } else { "meetings" };
        lines.push(format!("({waiting} more {noun} waiting)"));
    }
    card(
        Topic::Working,
        "Writing your notes",
        lines,
        vec![dismiss_button()],
    )
}

pub fn updating() -> Card {
    card(
        Topic::Updating,
        "Updating Meeting Notes",
        vec![
            "Meeting Notes will restart by itself.".to_string(),
            "This can take a few minutes.".to_string(),
        ],
        vec![dismiss_button()],
    )
}

/// The card for an update event, or `None` for the phases that need no words. The updater's own
/// text on a failure is technical, so it is not shown; the caller logs it.
pub fn update_card(phase: &str, message: &str) -> Option<Card> {
    let said = |fallback: &str| {
        let text = message.trim();
        if text.is_empty() { fallback } else { text }.to_string()
    };
    Some(match phase {
        "failed" => card(
            Topic::UpdateFailed,
            "The update failed.",
            vec![
                "Your current version still works.".to_string(),
                "Click Get help, then paste the steps into Claude.".to_string(),
            ],
            vec![
                button(Action::GetHelp, "Get help", true),
                report_button(),
                dismiss_button(),
            ],
        ),
        "refused" => notice(&said("The update did not start.")),
        "up_to_date" => notice(&said("You already have the newest version.")),
        "copied" => notice(&said("Copied.")),
        "timeout" => card(
            Topic::UpdateTimeout,
            "The update is taking too long.",
            vec!["You can keep using Meeting Notes.".to_string()],
            vec![report_button(), dismiss_button()],
        ),
        _ => return None,
    })
}

fn notice(title: &str) -> Card {
    card(
        Topic::UpdateNotice,
        title,
        Vec::new(),
        vec![dismiss_button()],
    )
}

pub fn meetings_problem(problem: MeetingsProblem) -> Card {
    let (title, hint) = match problem {
        MeetingsProblem::LinkMissing => (
            "The My Meetings link is missing.",
            "Ask Claude to repair the install, or use Report a problem.",
        ),
        MeetingsProblem::NotSetUp => (ErrorKind::Config.message(), ErrorKind::Config.hint()),
        MeetingsProblem::Browser => (
            "Could not open My Meetings.",
            "Your web browser did not open. Open claude.ai and look for My Meetings.",
        ),
    };
    card(
        Topic::MeetingsLink,
        title,
        vec![hint.to_string()],
        vec![report_button(), dismiss_button()],
    )
}

/// `names` are the shortcuts that could not be registered.
pub fn hotkeys_off(names: &[&str]) -> Card {
    let joined = names.join(" and ");
    let pronoun = if names.len() == 1 { "it" } else { "them" };
    auto_hiding(card(
        Topic::HotkeysOff,
        "Keyboard shortcuts are off.",
        vec![
            format!("{joined} could not be turned on. Another app may be using {pronoun}."),
            "The buttons on the widget still work.".to_string(),
        ],
        vec![dismiss_button()],
    ))
}

/// Why Start did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartRefusal {
    /// An update is running.
    Updating,
    /// A recording is already on.
    Busy,
    /// The recording thread could not be started: a fault, so it can be reported.
    Failed,
}

impl StartRefusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::Updating => "Meeting Notes is updating. Wait until it restarts.",
            Self::Busy => "A recording is already running.",
            Self::Failed => "Meeting Notes could not start the recording.",
        }
    }

    pub fn card(self) -> Card {
        start_refused(self.message(), self == Self::Failed)
    }
}

/// A folder (the log folder, or the one with the report) that could not be opened.
pub fn folder_failed(location: &str) -> Card {
    card(
        Topic::MeetingsLink,
        "Could not open the folder.",
        vec![format!("Find it here: {location}")],
        vec![dismiss_button()],
    )
}

/// `~/...` for a path inside the home folder, so the PM sees a short path and no user name is
/// spelled out.
pub fn home_relative(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

/// Start was refused (hotkey or button). `report` adds "Report a problem" for faults, not for
/// ordinary refusals such as an update in progress.
fn start_refused(message: &str, report: bool) -> Card {
    let mut buttons = Vec::new();
    if report {
        buttons.push(report_button());
    }
    buttons.push(dismiss_button());
    card(
        Topic::StartRefused,
        "Could not start the recording.",
        vec![message.to_string()],
        buttons,
    )
}

pub fn sound_test_running(seconds: u64) -> Card {
    card(
        Topic::SoundTestRunning,
        "Testing the sound",
        vec![format!("Talk, or play something, for {seconds} seconds.")],
        vec![dismiss_button()],
    )
}

pub fn sound_test_ok() -> Card {
    auto_hiding(card(
        Topic::SoundTestOk,
        "The sound check worked.",
        vec![
            "Meeting Notes heard sound.".to_string(),
            "Click Start when your meeting begins.".to_string(),
        ],
        vec![dismiss_button()],
    ))
}

pub fn report_working() -> Card {
    card(
        Topic::ReportWorking,
        "Creating the report",
        vec!["This takes a few seconds.".to_string()],
        vec![dismiss_button()],
    )
}

/// The report zip exists. `on_desktop` is false when the core fell back to the home folder.
pub fn report_ready(file_name: &str, on_desktop: bool) -> Card {
    let place = if on_desktop {
        "on your Desktop"
    } else {
        "in your home folder"
    };
    card(
        Topic::ReportReady,
        "Report ready",
        vec![
            format!("A report file is {place}: {file_name}"),
            "Send it to Mustafa in Teams.".to_string(),
        ],
        vec![
            button(Action::ShowFile, "Show the file", true),
            dismiss_button(),
        ],
    )
}

pub fn report_failed() -> Card {
    card(
        Topic::ReportFailed,
        "Could not create the report.",
        vec!["Open the log folder and send the files to Mustafa.".to_string()],
        vec![
            button(Action::OpenLogs, "Open log folder", true),
            dismiss_button(),
        ],
    )
}

/// What a click on the column's status mark shows: the most urgent thing first. `None` when
/// there is nothing to explain.
pub fn status_card(facts: &Facts) -> Option<Card> {
    if facts.updating {
        Some(updating())
    } else if let Some(kind) = facts.recording_failed {
        Some(recording_failed(kind))
    } else if !facts.failed.is_empty() {
        Some(notes_failed(&facts.failed))
    } else if let Some(stage) = &facts.processing {
        Some(working(stage, facts.waiting))
    } else if !facts.ready.is_empty() {
        Some(notes_ready(&facts.ready))
    } else {
        None
    }
}

pub fn notice_notes_failed(kind: ErrorKind) -> Notice {
    Notice {
        title: "Meeting notes could not be written".to_string(),
        body: kind.message().to_string(),
    }
}

pub fn notice_recording_failed(kind: ErrorKind) -> Notice {
    Notice {
        title: "Recording failed".to_string(),
        body: kind.message().to_string(),
    }
}

/// `label` is the session folder name.
pub fn notice_notes_ready(label: &str) -> Notice {
    Notice {
        title: format!("Notes ready - {}", your_meeting(label)),
        body: "Open My Meetings to read them.".to_string(),
    }
}

#[cfg(test)]
#[path = "card_tests.rs"]
mod tests;
