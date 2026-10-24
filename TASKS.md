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
  - [x] fixed size: resizing is turned off after the size is set (Linux: GTK size request = the size, because a non-resizable GTK window ignores set_size and stays 200x200). No resize cursors at the edges. Linux starts on a non-primary monitor when there is one, because Linux cannot hide the widget from screen sharing (Mac and Windows can, tested). Checked on Mustafa's 2 monitors via KWin: resizeable=false, HDMI-A-1, 44x170
  - [x] "Still recording" system notification at 30 min, then every 30 min (never auto-stops). `MN_REMIND_SECS` shortens it for tests. Unit-tested; live check pending on Linux, Windows, Mac
- [ ] 5. Mac + Windows (decided 2026-10-22: both now; tested on a test Mac and a Windows PC; PM test is 2026-10-23)
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
  - [ ] mac.sh on a test Mac (first real run), windows.ps1 on a Windows PC (first real run; also check start-at-login after a restart, the Run key path has a space)
  - [x] Mustafa's Arch autostart: tauri-plugin-autostart writes Exec= unquoted, and the dev path has spaces, so KDE skipped it. Exec now points at ~/.local/bin/meeting-notes. Toggling Start at login in the tray rewrites the broken path. AppImage installs are not affected (no spaces)
  - [x] KDE: the widget stayed on one virtual desktop. KWin ignores the sticky hint and _NET_WM_DESKTOP requests for it; a KWin window rule (class contains "eeting-notes", desktops = all, force) in ~/.config/kwinrulesrc fixes it. Set on Mustafa's Arch 2026-10-23
  - [ ] later: linux.sh adds that KWin rule when the desktop is KDE; check virtual desktops on Windows and Mac (Spaces) in the tests
  - [ ] later: quote the Linux Exec= path after enable() so dev paths with spaces work
- [ ] 7. Install on pilot Mac, run the pilot week
- [x] Transcription fix (v0.1.5): a 2026-10-23 Arabic meeting had 367 of 390 "Me" lines as two invented Arabic phrases, and the track took 10.5 min. Reproduced with a test track (45 s of noise, then Arabic): whisper picked Welsh and looped for the whole file. Fix: Silero VAD on by default (setup downloads ggml-silero-v5.1.2.bin, about 1 MB) with -vt 0.35 -vp 200 -vsd 500; -sns; -mc 0 when no vocab prompt; drop a 3+ word line after it appeared twice on a track; move the start of a >30 s segment to just before its end (VAD timestamp slip). Checked with the shipped b5130 Linux build: Arabic detected (p 0.9995), English after 40 s of noise detected, a 20 dB quieter speaker kept, 6x faster
  - [ ] later: languages mixed inside one track are still transcribed in the track's first language (whisper detects once per file)
- [ ] 8. Branding by account (after the Windows + Mac tests pass)
  - Claude account email ends with `@ctchealth.ch`: keep the ctcHealth logo, colours, and "CTC Health" in the prompts and the minutes .docx
  - Any other account: a generic look picked from https://styles.refero.design/ and no CTC name in prompts or .docx
  - The email comes from `claude auth status` at setup time; store it as a config key so the app and `mn init-page` can choose
- [ ] 9. Logging and usage analytics (design agreed first; see Decisions)
  - Local log file per machine for app + `mn` (widget.log exists; add mn runs, rotate at ~1 MB)
  - Central usage events, never meeting content: open questions to the user (service, whether events carry the user's email)
- [ ] 10. PMs install, update and reopen on their own (plan: `~/.claude-company/plans/you-have-to-realize-gleaming-cat.md`)
  - Done when, on a clean Mac and Windows, with no terminal typing: install with only Authorize/Allow clicks; update tag N to N+1 from the copied prompt; quit, reopen from the icon, autostart after a reboot
  - [x] 10a. No client or product names in the tree; root `*.docx`/`*.zip` ignored; history rewritten locally (backup bundle in the session scratchpad). Force-push waits for the user's go
  - [x] 10b. My Meetings page: "Ask MM" floating popup with the animated Storbi avatar; transcript buttons on the links/attendees row (worker)
  - [x] 10c. App: single instance, "Update available" tray item + widget badge that copies the update prompt, Linux autostart `Exec=` quoting (worker)
  - [x] 10d. `mn doctor` health check + `claude` fallback to `~/.local/bin` (worker)
  - [x] 10e. Setup scripts install git + Claude Code + `claude auth login`, `--update` flag, desktop/menu icons; `UPDATE.md`, `CHANGELOG.md`, `.claude/settings.json` allowlist, `scripts/release.sh`
  - [x] 10f. Cleaned history force-pushed (main, v0.1.4, v0.1.5 have no client names). v0.1.6 released 2026-10-24 (CI green, 7 files, marked Latest)
  - [ ] 10g. Real-machine test on Mac and Windows: install from the README sentence, count prompts, update v0.1.6 to a test v0.1.7, reopen from the Dock/Desktop icon, reboot. Untested so far: windows.ps1 in PowerShell 5.1 (no pwsh here), Xcode-tools flow, `claude auth login` from the agent shell, Dock tile, Desktop .lnk, the 404 "still being prepared" path, whether Desktop honours the repo and user permission rules
- [ ] 11. v0.1.7: back-to-back meetings and better outputs (asked 2026-10-24)
  - [x] 11a. The card uses Opus 5.5 at medium effort (config `card_model`, `card_effort`)
  - [x] 11b. Processing queue: Start works while the last meeting is still processing; a FIFO worker processes sessions in order; unfinished sessions resume after a restart (worker)
  - [x] 11c. One recording with back-to-back meetings is split into one card per meeting (`<session>-2` dirs), using the calendar and the transcript (worker)
  - [x] 11d. Ask MM buttons: "Email summary (Nat version)" and "Project kickoff (Eman style)". Built from the user's two examples (structure only; their content stays out of the public repo)
  - [x] 11e. Release v0.1.7 and republish the page (v0.1.7 tagged and pushed 2026-10-24)
- [ ] 12. One-click self-update, no Claude Code (v0.1.8)
  - Done when: clicking "Update v0.1.x" on the widget installs the newest release, restarts the app and says so; a failure restores the old version and offers "Get help" (copies the UPDATE.md sentence)
  - [x] 12a. `scripts/setup/update.sh` + `update.ps1`: fetch, newest tag, setup --update, mn doctor, rollback; status in `update-status.json` (worker)
  - [x] 12b. App: button shows the newest version, refuses while recording or writing notes, runs the updater detached, reports the result after restart (worker)
  - [ ] 12c. Release v0.1.8 before any PM installs (so nobody needs the copy-paste route)

## Decisions
- Capture interface: each capture source is a process that writes raw s16le 16 kHz mono PCM to stdout. On Linux this is `pw-record`; on Mac it will be a Swift sidecar. The Rust core reads the stream, measures levels, writes WAV, and auto-stops after silence.
- Speech-to-text: the whisper.cpp `whisper-cli`. On Linux it is built at `/mnt/data/audio-lab/whisper.cpp/build/bin/whisper-cli` and uses the `ggml-small` multilingual model at `/mnt/data/audio-lab/models/ggml/`.
- Headless artifact publishing needs `CLAUDE_CODE_ARTIFACT=1`. This env var is undocumented, so re-check it after Claude Code updates.
- M365 connector: works in `claude -p` on the CTCHealth account (`CLAUDE_CONFIG_DIR=~/.claude-company` on Arch). It shows needs-auth on the personal account.
- Two tracks, Me and Them. No speaker diarization in v1.
- Whisper: VAD is on by default since v0.1.5 (it was off after it dropped speech in an early test; a lower threshold, 0.35, and 200 ms padding keep quiet speech). Threads are set to all cores; that runs about 5.5× real time on this CPU.
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

- 2026-10-24, install/update for non-technical PMs (item 10):
  - PMs use Mac and Windows only. Hiding the widget from screen sharing on Linux is not done (no OS API); it is a known limit.
  - The repo stays public on the current account. A client and a product name were removed from all history again on 2026-10-24.
  - PMs only get tagged releases. main can move freely; `scripts/release.sh` bumps, tags, and CI builds.
  - git is required (setup installs it). The zip install path is gone.
  - Claude Desktop's sign-in is not shared with the `claude` CLI that `mn` uses, so setup installs Claude Code with the official installer and runs `claude auth login` (the PM clicks Authorize).
  - Updating: the app shows "Update available" and copies a prompt that points at `~/Meeting-Minutes/UPDATE.md`; the PM pastes it into Claude. Claude checks out the latest tag, re-runs setup with `--update`, runs `mn doctor`, and explains what is new from `CHANGELOG.md`.
  - The "Ask MM" popup uses the Storbi avatar (MM = the app's nickname, "my meetings").

## Blockers / open
- Using speakers instead of headphones means the mic re-records the other side, so lines get duplicated. v1 says "use headphones"; dedup is a later task.
