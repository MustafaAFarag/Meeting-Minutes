# TASKS: Meeting Notes widget (pilot)

Plan: `~/.claude/plans/super-high-level-quirky-tiger.md`

## Goal
The PM clicks Start when a call begins and it stops by itself when the call ends. Whisper transcribes locally and for free. The PM's own Claude (Codex as fallback) writes a meeting card to their private "My Meetings" claude.ai artifact.

## Done when
The pilot PM (Mac) uses it on 5 real meetings in 1 week. Each card is correct (right meeting name, usable to-dos) within 10 min of the call ending. The same app runs on Mustafa's Arch Linux.

## Checklist
- [x] 0. Spike (results in `docs/spike-results.md`)
- [ ] 1. Linux capture + transcribe CLI (`core/`, Rust): first pass reviewed, delta 1 in progress (adaptive silence, threads, garbage guard)
- [ ] 2. Meeting-card prompt + JSON schema: done and tested (`prompts/`). Rust summarize module + Claude→Codex fallback + queue: next
- [ ] 3. My Meetings artifact page (db: `meetings/<id>`, `transcripts/<id>`) + `scripts/push-meeting.sh`: in progress (subagent)
- [ ] 4. Tauri tray widget + 9:00 notification (Linux first)
- [ ] 5. Mac capture sidecar (Swift) + GitHub Actions release (.dmg, AppImage)
- [ ] 6. Repo CLAUDE.md + `setup` skill for beginner PMs
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

## Blockers / open
- Using speakers instead of headphones means the mic re-records the other side, so lines get duplicated. v1 says "use headphones"; dedup is a later task.
