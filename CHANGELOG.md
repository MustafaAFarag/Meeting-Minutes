# Changelog

Newest first. "What's new for you" is read by Claude to tell project managers what changed after an update, so keep it plain.
"Technical" is for developers. `scripts/release.sh` needs a `## vX.Y.Z` heading here before it makes a release, and replaces `unreleased` with the date.

## v0.1.8 - unreleased

### What's new for you
- Updating is one click: when the widget shows **Update** with a version number, click it. Meeting Notes installs the newest version, restarts by itself and tells you when it is done. No need to open Claude.
- If an update ever fails, your old version is put back and a **Get help** button appears.

### Technical
- `scripts/setup/update.sh` / `update.ps1`: fetch tags, check out the newest, `setup --update`, `mn doctor`, roll back on failure; status in `update-status.json`, log in `update.log` next to the config.
- App: `start_update` runs the copied script detached (own process group / DETACHED_PROCESS), refuses while recording or writing notes, polls the status file, reports the result after the restart (`self_update.rs`).

## v0.1.7 - 2026-10-24

### What's new for you
- You can start recording your next meeting while the last one is still being written up. Notes are written one after another, and a small badge shows the progress.
- If you forget to press Stop between two back-to-back meetings, you now get a separate card for each meeting.
- Ask MM has two new buttons: **Weekly team recap** (a friendly email to the team about the week) and **Project kick-off brief** (an internal brief from a kick-off meeting).
- Meeting cards are written by Claude Opus 5.5 for better notes.

### Technical
- App: recording and processing are separate; a FIFO queue (`app/src-tauri/src/queue.rs`) processes stopped sessions in order, keeps failures for Try again, and re-queues unfinished sessions at launch. New `mn-queue` event and `queue_status` command.
- Card schema returns 1-4 meetings with transcript ranges; extra meetings become `<session>-2` dirs with their own transcript slice and start time.
- Config `card_model` (default `claude-opus-5-5`) and `card_effort` (default `medium`) for the card.
- Publish prompt re-reads the page and republishes when the live version differs.

## v0.1.6 - 2026-10-24

### What's new for you
- **Ask MM:** on your My Meetings page you can now ask a question about your meetings and get an answer in a small popup, with Storbi.
- The transcript buttons on a meeting card now sit on one row, so the card is tidier.
- The widget tells you when a new version is ready (**Update available**). Click it and paste what it copies into Claude Code.
- Clicking the Meeting Notes icon when the widget is already running just shows it.
- After an install or update, Claude checks that everything works and fixes small problems for you.
- Installing is easier: git and the Claude sign-in are handled for you.

### Technical
- `mn doctor`: health check, one line per check and a final `DOCTOR_RESULT {json}` line.
- Setup scripts: `--update` / `-Update`, install the standalone Claude CLI with the official installer when missing, add a Dock tile (macOS), Desktop and Start-menu shortcuts (Windows) or an application-menu entry (Linux). `SETUP_RESULT` has `"update"`.
- Install location is fixed at `~/Meeting-Minutes` and PMs run tagged releases only (`UPDATE.md`, `scripts/release.sh`).
- Setup downloads the binaries of the release of the checked-out tag (rollback installs the old binaries); `SETUP_RESULT` has `"release"`.
- App: `tauri-plugin-single-instance`; daily GitHub latest-release check with a tray item and widget button that copy the update prompt (`MN_FAKE_LATEST` fakes it); Linux autostart `Exec=` is quoted and uses `$APPIMAGE`.
- `claude` falls back to `~/.local/bin/claude` when the bare `claude` is not on PATH.
- My Meetings page: Ask MM popup outside the re-rendered views, Storbi avatar runtime inlined (+65 KB).

## v0.1.5 - 2026-10-23

### What's new for you
- Better transcription: fewer made-up lines in quiet parts of a call.
- The language is picked from what is actually said, so a call that starts in silence is no longer mixed up.

### Technical
- Whisper VAD on by default (Silero model downloaded by the setup scripts); no invented repeat loops; language `auto` is decided from the first speech.
- The widget cannot be resized, and on Linux starts on the second monitor.

## v0.1.4 - 2026-10-23

### What's new for you
- First version that works on Mac, Windows and Linux.
- Press Play when a call starts and Stop when it ends. Your computer writes the text and Claude makes a meeting card on your private My Meetings page.

### Technical
- First release built by CI for macOS (universal), Windows and Linux. Earlier tags were removed when the repo history was cleaned for going public.
