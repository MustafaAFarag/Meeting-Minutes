# Building the installers

GitHub Actions builds the app for Mac, Windows and Linux. The installers are **unsigned**.

## Start a build

- **Test build (nothing is released):** GitHub → Actions → "Build installers" → Run workflow.
- **Release build:** bump `version` in `app/src-tauri/tauri.conf.json` (and `Cargo.toml`), then
  `git tag v0.1.0 && git push origin v0.1.0`. This publishes a GitHub Release (not a draft) and marks it
  as the latest one. It is public as soon as the build finishes, because the setup scripts download from
  `releases/latest/download/<name>`.

A build takes about 15 to 25 minutes the first time, about 8 to 12 once the cache is warm.

## Where the files are

Every file has a **fixed name** (no version in it). The setup scripts depend on these names, so do not rename them.

| OS | App | CLI (`mn`) | Whisper (`whisper-cli`) | Run page → Artifacts |
|----|-----|------------|-----------------------|----------------------|
| Mac | `MeetingNotes-macos.dmg` (universal) | `mn-macos` | `whisper-cli-macos` | `meeting-notes-macos` |
| Windows | `MeetingNotes-windows-setup.exe` | `mn-windows.exe` | — | `meeting-notes-windows` |
| Linux | `MeetingNotes-linux.AppImage` | `mn-linux` | — | `meeting-notes-linux` |

CI builds `whisper-cli-macos` from whisper.cpp tag `b5130` as one universal arm64 + x86_64 binary, with static libraries and embedded Metal shaders. CI checks both architectures, dynamic library dependencies, and a real transcription before publishing it.

On a `v*` tag the same seven files are attached to the published release (GitHub → Releases), and
`https://github.com/MustafaAFarag/Meeting-Minutes/releases/latest/download/<name>` serves them.
Artifacts are kept 90 days.

The installers come from `scripts/setup/` (see `SETUP.md`). To test the scripts without a release, point
them at a local folder that holds the six files: `--release-base <dir>` (Windows: `-ReleaseBase <dir>`).

## Install on a Mac (unsigned)

1. Open the `.dmg` and drag **Meeting Notes** into Applications.
2. Run once in Terminal: `xattr -dr com.apple.quarantine "/Applications/Meeting Notes.app"`
   (or right-click the app → **Open** → Open).
3. Start the app. Allow Microphone and "System Audio Recording" when macOS asks.

The app has no Dock icon. Look for the small tab at the screen edge and the menu bar icon.
Needs macOS 14.2 or newer.

## Install on Windows (unsigned)

1. Run the `-setup.exe`. It installs for your user only, no admin needed.
2. If SmartScreen says "Windows protected your PC": **More info** → **Run anyway**.

## Install on Linux

```
chmod +x "Meeting Notes_<version>_amd64.AppImage"
./"Meeting Notes_<version>_amd64.AppImage"
```

## Local builds

- Linux/Windows: `cd app/src-tauri && cargo tauri build` (needs `cargo install tauri-cli --version "^2" --locked`).
- Icons: `cargo tauri icon app-icon.png` regenerates `icons/` (including `icon.icns`) from the 1024×1024 source. CI does this on every build.
- Mac: the helper must exist first, because the bundle config lists it as `externalBin`.
  Run `bash mac/audiotap/build.sh`, then copy the printed file to
  `app/src-tauri/binaries/mn-audiotap-<triple>` (`aarch64-apple-darwin`, `x86_64-apple-darwin`,
  `universal-apple-darwin`). Without it, even `cargo build` fails on a Mac.
