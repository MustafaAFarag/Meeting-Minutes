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
- [ ] 5. Mac + Windows (decided 2026-10-22: both now; Mustafa tests on his brother's MacBook and his own Windows PC; PM test is 2026-10-23)
  - [x] A: `mn capture mic|system` via cpal (Windows loopback + mic, Mac mic), per-OS capture defaults, CREATE_NO_WINDOW, claude.cmd resolution
  - [x] B: `mac/audiotap` Swift helper for Mac system audio, Core Audio process tap
  - [x] C: Tauri bundling (.dmg / NSIS / AppImage) + `.github/workflows/build.yml`. Releases use fixed asset names
  - [x] `mn init-page` creates My Meetings on a fresh account (verified on the personal account, no M365)
  - [x] Pushed to github.com/MustafaAFarag/Meeting-Minutes (public), tag v0.1.0 → first CI release build (run 39820411166)
- [x] Ask Claude in My Meetings (minutes .docx in CTC format, recaps, free questions)
- [x] 6. One-sentence setup: `scripts/setup/{mac,linux,windows}`, SETUP.md, README, CLAUDE.md/AGENTS.md. linux.sh passed 2 runs in a sandboxed home
  - [ ] mac.sh on the brother's MacBook (first real run), windows.ps1 on Mustafa's PC (first real run)
  - [ ] real Linux install from the published release (downloads whisper + model)
- [ ] 7. Install on pilot Mac, run the pilot week

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

## Blockers / open
- Using speakers instead of headphones means the mic re-records the other side, so lines get duplicated. v1 says "use headphones"; dedup is a later task.
