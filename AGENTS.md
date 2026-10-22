If the user asks you to set up or install this, follow SETUP.md exactly.

# Meeting Notes: developer notes

## Layout

- `core/`: Rust library plus the `mn` CLI (record, transcribe, summarize, publish, init-page, process, run, config, capture). All the logic lives here.
- `app/`: the Tauri desktop widget (`app/src-tauri` Rust, `app/ui` HTML/JS). It uses `core` as a library.
- `artifact/index.html`: the "My Meetings" page that is published to the user's claude.ai.
- `mac/audiotap/`: the Swift helper that captures system audio on macOS (Core Audio process tap). Built into the app as `mn-audiotap`.
- `prompts/`: the meeting card prompt and its JSON schema.
- `scripts/setup/`: `mac.sh`, `linux.sh`, `windows.ps1`. Idempotent installers that SETUP.md drives. Keep them in step with the config keys in `core/src/config.rs`.
- `docs/building.md`: CI builds and releases. `TASKS.md`: current plan and decisions.

## Gates (before every commit)

Run in `core/` and in `app/src-tauri/`:

```
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

Scripts: `bash -n` on the `.sh` files (and `shellcheck` if installed). `windows.ps1` must stay ASCII only and work in Windows PowerShell 5.1 (no `&&`, no ternary, no `-Parallel`).

## Rules

- Never commit real meeting data (transcripts, cards, audio, URLs of real pages). The repo is public. Keep samples in `.local-samples/` (git-ignored).
- Release files have fixed names, because the setup scripts download `releases/latest/download/<name>`. Do not rename them without changing the scripts.
