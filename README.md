# Meeting Notes

A small widget on the edge of your screen. Press Play when a call starts, Stop when it ends.
Your computer turns the recording into text (nothing leaves it), and your own Claude writes a meeting card:
who was there, decisions, and a to-do list. The cards collect on a private "My Meetings" page in your claude.ai account.

## Install (one sentence)

Open Claude Code and say:

> download https://github.com/MustafaAFarag/Meeting-Minutes and set it up

Claude does the rest (about 1 GB download, 5 to 10 minutes). It asks for your full name once, installs everything, opens your
My Meetings page, and walks you through a 1-minute test.

## Privacy

- The recording stays on your computer and is deleted as soon as it is turned into text.
- The text and the cards go only to your own private page in your own claude.ai account. Nobody else sees them.
- Speech-to-text runs on your computer (whisper.cpp). No audio is uploaded.

## Requirements

| OS | Needs |
|---|---|
| macOS | 14.2 or newer (older: your microphone only, not the other side); Homebrew is only needed if the whisper download fails |
| Windows | Windows 10 or 11, 64-bit |
| Linux | 64-bit, PipeWire or PulseAudio (`pactl`, `parec`), libfuse2 for the AppImage |
| All | [Claude Code](https://claude.com/claude-code), logged in. About 1 GB of free disk. Headphones are best: with speakers your microphone records the other side twice |

## Manual install

Same script the AI runs. Clone the repo, then in a terminal in its folder:

- **macOS:** `bash scripts/setup/mac.sh --name "Your Name"`
- **Linux:** `bash scripts/setup/linux.sh --name "Your Name"`
- **Windows:** `powershell -ExecutionPolicy Bypass -File scripts\setup\windows.ps1 -Name "Your Name"`

Safe to run again: it updates in place and never touches your notes. Options: `--model small|turbo`, `--skip-page`,
`--skip-launch` (Windows: `-Model`, `-SkipPage`, `-SkipLaunch`). The app is not signed, so macOS or Windows may warn you once:
see [SETUP.md](SETUP.md) for what to click.

## Shortcuts

| Keys | Does |
|---|---|
| `Ctrl+Alt+R` (Mac: `Control+Option+R`) | Start / stop a recording |
| `Ctrl+Alt+M` (Mac: `Control+Option+M`) | Show / hide the tab |

The tab sits on the right screen edge: Play, Stop, Open My Meetings. It starts by itself when you log in.

## For developers

- Build the installers and publish a release: [docs/building.md](docs/building.md).
- Layout and rules for contributors and agents: [AGENTS.md](AGENTS.md).
- Gates before a commit: `cargo test`, `cargo clippy -- -D warnings` and `cargo fmt --check` in `core/` and in `app/src-tauri/`.
- This repo is public: never commit real meeting data.
