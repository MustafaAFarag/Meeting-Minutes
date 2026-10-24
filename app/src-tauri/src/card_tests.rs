use super::*;
use std::path::PathBuf;

fn labels(card: &Card) -> Vec<&str> {
    card.buttons.iter().map(|b| b.label.as_str()).collect()
}

fn actions(card: &Card) -> Vec<Action> {
    card.buttons.iter().map(|b| b.action).collect()
}

fn failed(name: &str, kind: ErrorKind) -> FailedItem {
    FailedItem {
        name: name.to_string(),
        kind,
    }
}

#[test]
fn a_session_label_is_only_the_date_time_name() {
    let path = PathBuf::from("/home/someone/Notes/2026-10-24_1405");
    assert_eq!(session_label(&path), "2026-10-24_1405");
    let split = PathBuf::from("/x/y/2026-10-24_1405-2");
    assert_eq!(session_label(&split), "2026-10-24_1405-2");
    // Any other folder name could carry a client name, so it never reaches the log.
    assert_eq!(session_label(Path::new("/x/Acme kickoff")), "session");
    assert_eq!(session_label(Path::new("/")), "session");
    assert_eq!(session_label(Path::new("/x/2026-10-24_1405-")), "session");
}

#[test]
fn the_meeting_time_comes_from_the_session_name() {
    assert_eq!(session_time("2026-10-24_1405").as_deref(), Some("14:05"));
    assert_eq!(session_time("2026-10-24_0905-3").as_deref(), Some("09:05"));
    assert_eq!(session_time("session"), None);
    assert_eq!(session_time("2026-10-24_2575"), None);
    assert_eq!(session_time(""), None);
}

#[test]
fn a_recording_that_heard_nothing_offers_a_short_test() {
    for kind in [ErrorKind::NoSound, ErrorKind::MicUnavailable] {
        let card = recording_failed(kind);
        assert_eq!(card.topic, Topic::RecordFailed);
        assert_eq!(card.title, kind.message());
        assert_eq!(card.lines, vec![kind.hint().to_string()]);
        assert_eq!(
            labels(&card),
            ["Try a short test", "Report a problem", "Dismiss"]
        );
        assert_eq!(
            actions(&card),
            [Action::ShortTest, Action::Report, Action::Dismiss]
        );
        assert!(card.buttons[0].primary);
    }
}

#[test]
fn another_recording_failure_says_start_again_not_try_again() {
    let card = recording_failed(ErrorKind::Unknown);
    assert_eq!(labels(&card)[0], "Start again");
    assert_eq!(actions(&card)[0], Action::StartAgain);
    assert!(!card.lines[0].contains("Try again"), "{:?}", card.lines);
    assert!(card.lines[0].contains("Start again"));
}

#[test]
fn failed_notes_name_the_meeting_by_its_time_only() {
    let card = notes_failed(&[failed("2026-10-24_1405", ErrorKind::ClaudeSignedOut)]);
    assert_eq!(card.topic, Topic::NotesFailed);
    assert_eq!(
        card.title,
        "Notes for your 14:05 meeting could not be written."
    );
    assert_eq!(
        card.lines,
        vec![
            ErrorKind::ClaudeSignedOut.message().to_string(),
            ErrorKind::ClaudeSignedOut.hint().to_string()
        ]
    );
    assert_eq!(
        actions(&card),
        [Action::Retry, Action::Report, Action::Dismiss]
    );
    assert_eq!(labels(&card), ["Try again", "Report a problem", "Dismiss"]);
}

#[test]
fn a_meeting_with_no_readable_time_is_just_your_meeting() {
    let card = notes_failed(&[failed("session", ErrorKind::Whisper)]);
    assert_eq!(card.title, "Notes for your meeting could not be written.");
}

#[test]
fn notes_that_were_written_but_not_published_say_so() {
    let card = notes_failed(&[failed("2026-10-24_1405", ErrorKind::Publish)]);
    assert_eq!(card.title, "Your 14:05 meeting is not on My Meetings yet.");
    assert_eq!(card.lines[0], ErrorKind::Publish.message());
}

#[test]
fn several_failed_meetings_become_one_card() {
    let card = notes_failed(&[
        failed("2026-10-24_0905", ErrorKind::Network),
        failed("2026-10-24_1405", ErrorKind::ClaudeSignedOut),
    ]);
    assert_eq!(card.title, "2 meetings need attention");
    assert_eq!(labels(&card)[0], "Try again all");
    assert_eq!(card.lines.len(), 3);
    assert_eq!(
        card.lines[0],
        format!("09:05 meeting: {}", ErrorKind::Network.message())
    );
    // The next step is the one for the newest failure.
    assert_eq!(card.lines[2], ErrorKind::ClaudeSignedOut.hint());
}

#[test]
fn a_long_list_of_failed_meetings_is_cut_short() {
    let items: Vec<_> = (0..6)
        .map(|n| failed(&format!("2026-10-24_100{n}"), ErrorKind::Unknown))
        .collect();
    let card = notes_failed(&items);
    assert_eq!(card.title, "6 meetings need attention");
    // Two meetings, "and 4 more", and the next step: the card is only so tall.
    assert_eq!(card.lines.len(), 4);
    assert_eq!(card.lines[2], "and 4 more");
    assert_eq!(card.lines[3], ErrorKind::Unknown.hint());
}

#[test]
fn ready_notes_say_where_they_are_and_hide_after_20_seconds() {
    let card = notes_ready(&["2026-10-24_1405".to_string()]);
    assert_eq!(card.topic, Topic::NotesReady);
    assert_eq!(card.title, "Notes ready");
    assert_eq!(card.lines, vec!["Your 14:05 meeting is on My Meetings."]);
    assert_eq!(labels(&card), ["Open My Meetings"]);
    assert_eq!(card.auto_hide_ms, Some(20_000));
    let two = notes_ready(&["2026-10-24_1405".into(), "2026-10-24_1500".into()]);
    assert_eq!(two.lines, vec!["2 meetings are on My Meetings."]);
}

#[test]
fn working_shows_the_stage_the_usual_time_and_who_waits() {
    let card = working("Turning speech into text", 0);
    assert_eq!(card.topic, Topic::Working);
    assert_eq!(card.title, "Writing your notes");
    assert_eq!(
        card.lines,
        vec!["Turning speech into text", "Usually 2-5 minutes"]
    );
    assert_eq!(card.auto_hide_ms, None);
    assert_eq!(
        working("Writing the notes", 1).lines[2],
        "(1 more meeting waiting)"
    );
    assert_eq!(
        working("Writing the notes", 3).lines[2],
        "(3 more meetings waiting)"
    );
}

#[test]
fn only_errors_are_alert_dialogs() {
    let alert = [
        Topic::RecordFailed,
        Topic::NotesFailed,
        Topic::UpdateFailed,
        Topic::UpdateTimeout,
        Topic::MeetingsLink,
        Topic::StartRefused,
        Topic::ReportFailed,
    ];
    let status = [
        Topic::NotesReady,
        Topic::Working,
        Topic::Updating,
        Topic::UpdateNotice,
        Topic::HotkeysOff,
        Topic::SoundTestRunning,
        Topic::SoundTestOk,
        Topic::ReportWorking,
        Topic::ReportReady,
    ];
    for topic in alert {
        assert_eq!((topic.role(), topic.tone()), ("alertdialog", "error"));
    }
    for topic in status {
        assert_eq!(topic.role(), "status", "{topic:?}");
        assert_ne!(topic.tone(), "error", "{topic:?}");
    }
    assert_eq!(Topic::NotesReady.tone(), "ok");
}

#[test]
fn every_card_has_a_title_and_a_way_to_close_it() {
    let cards = [
        recording_failed(ErrorKind::NoSound),
        notes_failed(&[failed("2026-10-24_1405", ErrorKind::Unknown)]),
        working("Writing the notes", 0),
        updating(),
        meetings_problem(MeetingsProblem::LinkMissing),
        meetings_problem(MeetingsProblem::NotSetUp),
        meetings_problem(MeetingsProblem::Browser),
        hotkeys_off(&["Ctrl+Alt+M"]),
        start_refused("A recording is already running.", true),
        sound_test_running(5),
        sound_test_ok(),
        report_working(),
        report_ready("MeetingNotes-report-2026-10-24_1405.zip", true),
        report_failed(),
        update_card("failed", "x").unwrap(),
        update_card("timeout", "").unwrap(),
    ];
    for card in cards {
        assert!(!card.title.is_empty(), "{:?}", card.topic);
        assert!(
            actions(&card).contains(&Action::Dismiss),
            "{:?} has no Dismiss",
            card.topic
        );
        assert_eq!(card.role, card.topic.role());
        assert_eq!(card.tone, card.topic.tone());
    }
}

#[test]
fn the_column_mark_explains_the_most_urgent_thing() {
    let mut facts = Facts::default();
    assert_eq!(status_card(&facts), None);
    facts.ready = vec!["2026-10-24_1405".into()];
    assert_eq!(status_card(&facts).unwrap().topic, Topic::NotesReady);
    facts.processing = Some("Writing the notes".into());
    assert_eq!(status_card(&facts).unwrap().topic, Topic::Working);
    facts.failed = vec![failed("2026-10-24_1405", ErrorKind::Whisper)];
    assert_eq!(status_card(&facts).unwrap().topic, Topic::NotesFailed);
    facts.recording_failed = Some(ErrorKind::NoSound);
    assert_eq!(status_card(&facts).unwrap().topic, Topic::RecordFailed);
    facts.updating = true;
    assert_eq!(status_card(&facts).unwrap().topic, Topic::Updating);
}

#[test]
fn the_working_card_comes_from_the_queue_numbers() {
    let facts = Facts {
        processing: Some("Adding to My Meetings".into()),
        waiting: 2,
        ..Facts::default()
    };
    let card = status_card(&facts).unwrap();
    assert_eq!(card.lines[0], "Adding to My Meetings");
    assert_eq!(card.lines[2], "(2 more meetings waiting)");
}

#[test]
fn update_words_move_into_the_card() {
    let failed = update_card("failed", "mn doctor exited 2").unwrap();
    assert_eq!(failed.topic, Topic::UpdateFailed);
    // The updater's own text is technical: it goes to the log, not to the card.
    let all = format!("{} {}", failed.title, failed.lines.join(" "));
    assert!(!all.contains("mn doctor"), "{all}");
    assert_eq!(
        actions(&failed),
        [Action::GetHelp, Action::Report, Action::Dismiss]
    );

    let refused = update_card("refused", "Stop the recording first, then update.").unwrap();
    assert_eq!(refused.title, "Stop the recording first, then update.");
    assert_eq!(refused.topic, Topic::UpdateNotice);
    assert_eq!(actions(&refused), [Action::Dismiss]);

    let same = update_card("up_to_date", "You already have the newest version.").unwrap();
    assert_eq!(same.title, "You already have the newest version.");
    let copied = update_card("copied", "Copied. Paste it into Claude.").unwrap();
    assert_eq!(copied.title, "Copied. Paste it into Claude.");

    let slow = update_card("timeout", "").unwrap();
    assert_eq!(slow.title, "The update is taking too long.");
    assert_eq!(actions(&slow), [Action::Report, Action::Dismiss]);

    for quiet in ["offer", "updating", "quiet", "nonsense"] {
        assert_eq!(update_card(quiet, "x"), None, "{quiet}");
    }
}

#[test]
fn a_message_less_notice_still_has_words() {
    assert_eq!(
        update_card("refused", "").unwrap().title,
        "The update did not start."
    );
    assert_eq!(update_card("copied", "").unwrap().title, "Copied.");
    assert_eq!(
        update_card("up_to_date", "").unwrap().title,
        "You already have the newest version."
    );
}

#[test]
fn meetings_problems_have_plain_words_and_a_next_step() {
    let missing = meetings_problem(MeetingsProblem::LinkMissing);
    assert_eq!(missing.title, "The My Meetings link is missing.");
    assert_eq!(actions(&missing), [Action::Report, Action::Dismiss]);
    let set_up = meetings_problem(MeetingsProblem::NotSetUp);
    assert_eq!(set_up.title, ErrorKind::Config.message());
    assert_eq!(set_up.lines, vec![ErrorKind::Config.hint().to_string()]);
    let browser = meetings_problem(MeetingsProblem::Browser);
    assert_eq!(browser.title, "Could not open My Meetings.");
}

#[test]
fn hotkeys_off_names_the_keys_that_failed() {
    let card = hotkeys_off(&["Ctrl+Alt+M", "Ctrl+Alt+R"]);
    assert_eq!(card.title, "Keyboard shortcuts are off.");
    assert!(card.lines[0].starts_with("Ctrl+Alt+M and Ctrl+Alt+R could not be turned on"));
    assert!(card.lines[1].contains("buttons"));
    assert_eq!(card.auto_hide_ms, Some(20_000));
    let one = hotkeys_off(&["Ctrl+Alt+R"]);
    assert!(one.lines[0].starts_with("Ctrl+Alt+R could not be turned on"));
}

#[test]
fn a_refused_start_shows_why() {
    let card = start_refused("A recording is already running.", false);
    assert_eq!(card.title, "Could not start the recording.");
    assert_eq!(card.lines, vec!["A recording is already running."]);
    assert_eq!(actions(&card), [Action::Dismiss]);
    let with_report = start_refused("x", true);
    assert_eq!(actions(&with_report), [Action::Report, Action::Dismiss]);
}

#[test]
fn the_report_card_points_at_the_file_and_at_teams() {
    let name = "MeetingNotes-report-2026-10-24_1405.zip";
    let card = report_ready(name, true);
    assert_eq!(
        card.lines[0],
        format!("A report file is on your Desktop: {name}")
    );
    assert_eq!(card.lines[1], "Send it to Mustafa in Teams.");
    assert_eq!(actions(&card), [Action::ShowFile, Action::Dismiss]);
    assert_eq!(labels(&card)[0], "Show the file");
    let home = report_ready(name, false);
    assert!(home.lines[0].starts_with("A report file is in your home folder: "));

    let bad = report_failed();
    assert_eq!(bad.title, "Could not create the report.");
    assert_eq!(
        bad.lines,
        vec!["Open the log folder and send the files to Mustafa."]
    );
    assert_eq!(labels(&bad)[0], "Open log folder");
}

#[test]
fn notifications_carry_the_plain_message() {
    let notes = notice_notes_failed(ErrorKind::Network);
    assert_eq!(notes.title, "Meeting notes could not be written");
    assert_eq!(notes.body, ErrorKind::Network.message());
    let rec = notice_recording_failed(ErrorKind::NoSound);
    assert_eq!(rec.title, "Recording failed");
    assert_eq!(rec.body, ErrorKind::NoSound.message());
    let ready = notice_notes_ready("2026-10-24_1405");
    assert_eq!(ready.title, "Notes ready - your 14:05 meeting");
    assert_eq!(
        notice_notes_ready("session").title,
        "Notes ready - your meeting"
    );
}

#[test]
fn button_names_parse_back_to_actions() {
    for action in [
        Action::Dismiss,
        Action::Retry,
        Action::StartAgain,
        Action::ShortTest,
        Action::Report,
        Action::OpenMeetings,
        Action::ShowFile,
        Action::OpenLogs,
        Action::GetHelp,
    ] {
        let name = serde_json::to_value(action).unwrap();
        assert_eq!(Action::parse(name.as_str().unwrap()), Some(action));
    }
    assert_eq!(Action::parse("format_disk"), None);
}

#[test]
fn the_window_gets_plain_json() {
    let value = serde_json::to_value(recording_failed(ErrorKind::NoSound)).unwrap();
    assert_eq!(value["topic"], "record_failed");
    assert_eq!(value["role"], "alertdialog");
    assert_eq!(value["buttons"][0]["action"], "short_test");
    assert_eq!(value["buttons"][0]["primary"], true);
    assert!(value["auto_hide_ms"].is_null());
}

#[test]
fn an_error_card_is_not_replaced_by_good_news() {
    let mut slot = Slot::default();
    let error = recording_failed(ErrorKind::NoSound);
    assert!(slot.offer(error.clone(), false).is_some());
    assert!(slot
        .offer(notes_ready(&["2026-10-24_1405".into()]), false)
        .is_none());
    assert_eq!(slot.current(), Some(&error));
    // A newer error replaces it, and a click (force) replaces anything.
    assert!(slot
        .offer(
            notes_failed(&[failed("2026-10-24_1405", ErrorKind::Network)]),
            false
        )
        .is_some());
    assert_eq!(slot.current().unwrap().topic, Topic::NotesFailed);
    assert!(slot.offer(working("Writing the notes", 0), true).is_some());
    assert_eq!(slot.current().unwrap().topic, Topic::Working);
}

#[test]
fn the_working_card_is_refreshed_by_the_same_topic() {
    let mut slot = Slot::default();
    slot.offer(working("Turning speech into text", 0), false);
    assert!(slot.offer(working("Writing the notes", 0), false).is_some());
    assert_eq!(slot.current().unwrap().lines[0], "Writing the notes");
}

#[test]
fn an_old_timer_cannot_close_a_newer_card() {
    let mut slot = Slot::default();
    let first = slot
        .offer(notes_ready(&["2026-10-24_1405".into()]), false)
        .unwrap();
    let second = slot
        .offer(notes_ready(&["2026-10-24_1500".into()]), false)
        .unwrap();
    assert_ne!(first, second);
    assert!(!slot.clear_if(first));
    assert!(slot.current().is_some());
    assert!(slot.clear_if(second));
    assert!(slot.current().is_none());
}

#[test]
fn clearing_returns_what_was_closed() {
    let mut slot = Slot::default();
    assert_eq!(slot.clear(), None);
    slot.offer(updating(), false);
    assert_eq!(slot.clear().unwrap().topic, Topic::Updating);
    assert!(slot.current().is_none());
}

/// Every card the app can show, by name. The window check renders these (see `dump_every_card`).
fn every_card() -> Vec<(&'static str, Card)> {
    let one = |kind| notes_failed(&[failed("2026-10-24_1405", kind)]);
    let many: Vec<_> = (0..6)
        .map(|n| failed(&format!("2026-10-24_100{n}"), ErrorKind::ClaudeSignedOut))
        .collect();
    vec![
        ("record_no_sound", recording_failed(ErrorKind::NoSound)),
        ("record_mic", recording_failed(ErrorKind::MicUnavailable)),
        ("record_unknown", recording_failed(ErrorKind::Unknown)),
        ("notes_one", one(ErrorKind::ClaudeSignedOut)),
        ("notes_publish", one(ErrorKind::Publish)),
        ("notes_many", notes_failed(&many)),
        ("notes_ready", notes_ready(&["2026-10-24_1405".to_string()])),
        ("working", working("Turning speech into text", 0)),
        ("working_waiting", working("Adding to My Meetings", 2)),
        ("updating", updating()),
        ("update_failed", update_card("failed", "x").unwrap()),
        (
            "update_refused",
            update_card(
                "refused",
                "Your notes are still being written. Update when they are done.",
            )
            .unwrap(),
        ),
        ("update_timeout", update_card("timeout", "").unwrap()),
        (
            "meetings_missing",
            meetings_problem(MeetingsProblem::LinkMissing),
        ),
        (
            "meetings_browser",
            meetings_problem(MeetingsProblem::Browser),
        ),
        ("hotkeys_off", hotkeys_off(&["Ctrl+Alt+M", "Ctrl+Alt+R"])),
        ("start_refused", StartRefusal::Failed.card()),
        ("sound_test_running", sound_test_running(5)),
        ("sound_test_ok", sound_test_ok()),
        ("report_working", report_working()),
        (
            "report_ready",
            report_ready("MeetingNotes-report-2026-10-24_1405.zip", true),
        ),
        ("report_failed", report_failed()),
        (
            "folder_failed",
            folder_failed("~/Library/Logs/MeetingNotes"),
        ),
    ]
}

#[test]
fn every_card_fits_the_card_column_in_words() {
    // The card is 256 px wide at 11.5 px text: about 38 characters a line. A line far beyond
    // what four lines hold would not fit the 300 px tall window.
    for (name, card) in every_card() {
        for line in card.lines.iter().chain(std::iter::once(&card.title)) {
            assert!(line.chars().count() <= 160, "{name}: {line}");
        }
        assert!(card.lines.len() <= 5, "{name}");
        assert!(card.buttons.len() <= 3, "{name}");
    }
}

/// Writes every card as JSON to `target/ui-cards.json` (ignored by git), so a browser check of the
/// window can draw the real texts and buttons. Also proves they all serialise.
#[test]
fn dump_every_card_for_the_window_check() {
    let map: serde_json::Map<String, serde_json::Value> = every_card()
        .into_iter()
        .map(|(name, card)| (name.to_string(), serde_json::to_value(card).unwrap()))
        .collect();
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ui-cards.json");
    if path.parent().is_some_and(Path::is_dir) {
        std::fs::write(path, serde_json::to_string_pretty(&map).unwrap()).unwrap();
    }
}

#[test]
fn each_refusal_to_start_has_plain_words() {
    let updating = StartRefusal::Updating.card();
    assert_eq!(updating.title, "Could not start the recording.");
    assert_eq!(
        updating.lines,
        vec!["Meeting Notes is updating. Wait until it restarts."]
    );
    assert_eq!(actions(&updating), [Action::Dismiss]);
    let busy = StartRefusal::Busy.card();
    assert_eq!(busy.lines, vec!["A recording is already running."]);
    assert_eq!(actions(&busy), [Action::Dismiss]);
    // A fault, not a rule: it can be reported.
    let fault = StartRefusal::Failed.card();
    assert_eq!(actions(&fault), [Action::Report, Action::Dismiss]);
    assert_eq!(StartRefusal::Failed.message(), fault.lines[0]);
}

#[test]
fn a_folder_that_will_not_open_says_where_it_is() {
    let card = folder_failed("~/Library/Logs/MeetingNotes");
    assert_eq!(card.title, "Could not open the folder.");
    assert_eq!(
        card.lines,
        vec!["Find it here: ~/Library/Logs/MeetingNotes"]
    );
    assert_eq!(actions(&card), [Action::Dismiss]);
}

#[test]
fn paths_in_the_home_folder_are_shown_with_a_tilde() {
    let home = PathBuf::from("/home/someone");
    assert_eq!(
        home_relative(Path::new("/home/someone/.cache/meeting-notes"), Some(&home)),
        "~/.cache/meeting-notes"
    );
    assert_eq!(home_relative(Path::new("/home/someone"), Some(&home)), "~");
    assert_eq!(
        home_relative(Path::new("/var/log/x"), Some(&home)),
        "/var/log/x"
    );
    assert_eq!(home_relative(Path::new("/var/log/x"), None), "/var/log/x");
}
