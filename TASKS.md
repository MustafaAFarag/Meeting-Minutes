# TASKS: Meeting Notes widget (pilot)

Plan: `~/.claude/plans/super-high-level-quirky-tiger.md`

## Goal
The PM clicks Start when a call begins and it stops by itself when the call ends. Whisper transcribes locally and for free. The PM's own Claude (Codex as fallback) writes a meeting card to their private "My Meetings" claude.ai artifact.

## Done when
The pilot PM (Mac) uses it on 5 real meetings in 1 week. Each card is correct (right meeting name, usable to-dos) within 10 min of the call ending. The same app runs on Mustafa's Arch Linux.

## Checklist
- [x] 0. Spike (results in `docs/spike-results.md`)
- [x] 1. Linux capture + transcribe CLI (`core/`, Rust): parec two-track capture, adaptive silence auto-stop, whisper.cpp, audio deletion. Tested live on Arch
- [x] 2. Card prompt + schema, `mn summarize` (Claude → Codex fallback), `mn publish` (pending-marker queue), `mn process`, `mn run`. Tested end to end with real Claude, Codex and a real artifact
- [x] 3. My Meetings artifact page: full-width master-detail layout, ctcHealth brand, ticks in db. The user confirmed ticks persist
- [ ] 4. Tauri tray widget + 9:00 notification (Linux first)
  - [x] auto-stop removed; plain widget (Start/levels/Stop/spinner/Done) — user confirmed it works
  - [x] notch pill at top edge, hover unfold, drag + remembered position, Ctrl+Alt+M show/hide, Ctrl+Alt+R record toggle, start at login (tray toggle). Folded look checked by screenshot; unfold/drag/hotkeys need the user's check
  - [ ] 9:00 notification
  - [x] "Still recording" system notification at 30 min, then every 30 min (never auto-stops). `MN_REMIND_SECS` shortens it for tests. Unit-tested; live check pending on Linux, Windows, Mac
- [ ] 5. Mac + Windows (decided 2026-10-22: both now; Mustafa tests on his brother's MacBook and his own Windows PC; PM test is 2026-10-23)
  - [x] A: `mn capture mic|system` via cpal (Windows loopback + mic, Mac mic), per-OS capture defaults, CREATE_NO_WINDOW, claude.cmd resolution
  - [x] B: `mac/audiotap` Swift helper for Mac system audio, Core Audio process tap
  - [x] C: Tauri bundling (.dmg / NSIS / AppImage) + `.github/workflows/build.yml`. Releases use fixed asset names
  - [x] `mn init-page` creates My Meetings on a fresh account (verified on the personal account, no M365)
  - [x] Pushed to github.com/MustafaAFarag/Meeting-Minutes (public). v0.1.0 build failed (globstar in collect step; macOS /var vs /private/var in publish.rs:164); both fixed
  - [x] v0.1.1: Windows green; Mac failed at the whisper otool check (header per arch); Linux apt hung 4 h (cancelled). Fixed
  - [x] v0.1.2: Windows green; Mac passed whisper check + tests + installer, failed at Collect (`shopt globstar` missing in bash 3.2); Linux apt timed out at 10 min (slow azure.archive.ubuntu.com). Fixed: /usr/bin/find lookup; awalsh128/cache-apt-pkgs-action@v1.6.3 (apt-fast), 20 min limit
  - [x] **v0.1.3 released 2026-10-23** (run green on all 3 OS, 23 min): 7 files at releases/latest/download/. Release step still warns softprops/action-gh-release@v2 is Node 20 (v3 exists; not changed)
- [x] Ask Claude in My Meetings (minutes .docx in CTC format, recaps, free questions)
- [x] 6. One-sentence setup: `scripts/setup/{mac,linux,windows}`, SETUP.md, README, CLAUDE.md/AGENTS.md. linux.sh passed 2 runs in a sandboxed home
  - [x] name auto-detect (`--detect-name`: Mac account, Windows Office/365 name, Linux GECOS, Claude email guess); agent asks "Is your name X?"
  - [x] Mac whisper built in CI (`whisper-cli-macos`, universal, Metal); Homebrew only as fallback
  - [x] SETUP.md step 3 checks the M365 connector via `claude mcp list` (never blocks)
  - [x] real Linux install from the v0.1.3 release in a sandboxed HOME: ok in 3 min, downloaded whisper transcribes jfk.wav, re-run idempotent. Fixed a false "libfuse2 not found" (grep -q + pipefail SIGPIPE)
  - [ ] mac.sh on the brother's MacBook (first real run), windows.ps1 on Mustafa's PC (first real run; also check start-at-login after a restart, the Run key path has a space)
  - [x] Mustafa's Arch autostart: tauri-plugin-autostart writes Exec= unquoted, and the dev path has spaces, so KDE skipped it. Exec now points at ~/.local/bin/meeting-notes. Toggling Start at login in the tray rewrites the broken path. AppImage installs are not affected (no spaces)
  - [x] KDE: the widget stayed on one virtual desktop. KWin ignores the sticky hint and _NET_WM_DESKTOP requests for it; a KWin window rule (class contains "eeting-notes", desktops = all, force) in ~/.config/kwinrulesrc fixes it. Set on Mustafa's Arch 2026-10-23
  - [ ] later: linux.sh adds that KWin rule when the desktop is KDE; check virtual desktops on Windows and Mac (Spaces) in the tests
  - [ ] later: quote the Linux Exec= path after enable() so dev paths with spaces work
- [ ] 7. Install on pilot Mac, run the pilot week
- [ ] 8. Branding by account (after the Windows + Mac tests pass)
  - Claude account email ends with `@ctchealth.ch`: keep the ctcHealth logo, colours, and "CTC Health" in the prompts and the minutes .docx
  - Any other account: a generic look picked from https://styles.refero.design/ and no CTC name in prompts or .docx
  - The email comes from `claude auth status` at setup time; store it as a config key so the app and `mn init-page` can choose
- [ ] 9. Logging and usage analytics (design agreed first; see Decisions)
  - Local log file per machine for app + `mn` (widget.log exists; add mn runs, rotate at ~1 MB)
  - Central usage events, never meeting content: open questions to the user (service, whether events carry the user's email)

## Decisions
- Capture interface: each capture source is a process that writes raw s16le 16 kHz mono PCM to stdout. On Linux this is `pw-record`; on Mac it will be a Swift sidecar. The Rust core reads the stream, measures levels, writes WAV, and auto-stops after silence.
- Speech-to-text: the whisper.cpp `whisper-cli`. On Linux it is built at `/mnt/data/audio-lab/whisper.cpp/build/bin/whisper-cli` and uses the `ggml-small` multilingual model at `/mnt/data/audio-lab/models/ggml/`.
- Headless artifact publishing needs `CLAUDE_CODE_ARTIFACT=1`. This env var is undocumented, so re-check it after Claude Code updates.
- M365 connector: works in `claude -p` on the CTCHealth account (`CLAUDE_CONFIG_DIR=~/.claude-company` on Arch). It shows needs-auth on the personal account.
- Two tracks, Me and Them. No speaker diarization in v1.
- Whisper: VAD stays off by default because it dropped real speech in the test. Threads are set to all cores; that runs about 5.5× real time on this CPU.
- Card generation: `claude -p --json-schema` / `codex exec --output-schema`. Both take about 40 s. Claude+M365 matched the calendar event and found real links.
- The repo is public: never commit real cards or transcripts. Keep them in `.local-samples/` (git-ignored).

- Page data is published as files (`data/index.json`, `data/meetings/<id>.json`, `data/transcripts/<id>.md`), because headless ArtifactData writes need an interactive consent that no allow rule bypasses. The db holds only ticks, written by the viewer in the browser.
- Capture uses `parec`, not `pw-record`: WirePlumber routed both pw-record streams to the sink monitor even with `--target`.
- Whisper model per machine: small on the Arch CPU (about 3× real time), large-v3-turbo planned for the Mac (GPU; speed not measured yet).
- Timing: summarizing takes about 40–60 s, publishing about 30–40 s.

- 2026-10-22, after the first real test call: **auto-stop is removed**. Recording stops only when the user presses Stop in the widget (or Ctrl+C / STOP file). The reason: mic noise such as typing kept resetting the silence timer. The widget is Start → recording timer → Stop → rotating icon "Transcribing… / Writing your notes… / Updating My Meetings…" → Done.
- First real call (3 min, script): calendar title matched, 6 to-dos (5 English + 1 Arabic), decisions and deliverables correct, page auto-updated. Small misses: "chase agency" got a date (30 Oct) instead of none; Arabic line 17 (colleague reviews Monday) was not in waiting-on.

- 2026-10-23: the repo is public. Removed from all history: the real My Meetings and spike page links, and a real internal project name (plus a mock one) from the test call script. History was rewritten with git-filter-repo; old tags v0.1.0 to v0.1.3 are replaced by v0.1.4.
- Analytics must never send transcripts, titles, attendees, card text or page links. Only counts, durations, success/failure, error class, OS, app version.

## Blockers / open
- Using speakers instead of headphones means the mic re-records the other side, so lines get duplicated. v1 says "use headphones"; dedup is a later task.
