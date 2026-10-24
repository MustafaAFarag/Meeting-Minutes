# Logs and problem reports

Core and the desktop widget share `meeting-notes.log`. It rotates at 1 MB and keeps
`meeting-notes.log.1` and `meeting-notes.log.2`.

- macOS: `~/Library/Logs/MeetingNotes`
- Windows: `%LOCALAPPDATA%\MeetingNotes\logs`
- Linux: `~/.cache/meeting-notes`

The old `widget.log` is still included in reports. Update logs stay in the
`meeting-notes` folder under the OS config directory.

Run `mn report` to create `MeetingNotes-report-<date-time>.zip` on the Desktop. If
the Desktop folder is unavailable, it uses the home folder. The archive contains
logs, version and installation details, notes counts, the `mn doctor` report, and
a redacted config. It does not include meeting audio, transcripts, cards, or other
session files. `MN_LOG_DIR` can point logs at a temporary folder for local tests.
