# SETUP: runbook for the AI agent

The user said something like "download https://github.com/MustafaAFarag/Meeting-Minutes and set it up".
The user is a non-technical project manager. Do everything yourself. Ask only the one question in step 3.
Speak plainly: no jargon, no stack traces. Show an error line only when this file says to.

**Done means:** the widget is running, `SETUP_RESULT` said `"ok":true`, the My Meetings page opened, and the 1-minute test in step 8 produced a card.
**Stop and report** if a step fails twice in the same way. Say what failed and what the user can do.

## 0. Get the files

If the repo is not on the computer yet:

- macOS / Linux: `git clone https://github.com/MustafaAFarag/Meeting-Minutes ~/Meeting-Minutes`
- Windows (PowerShell): `git clone https://github.com/MustafaAFarag/Meeting-Minutes $HOME\Meeting-Minutes`
- No `git`: download `https://github.com/MustafaAFarag/Meeting-Minutes/archive/refs/heads/main.zip` and unzip it.

If it is already there, run `git pull` inside it. Run every command below from that folder.

## 1. Detect the OS

| Shell says | OS | Script |
|---|---|---|
| `uname -s` = `Darwin` | macOS | `scripts/setup/mac.sh` |
| `uname -s` = `Linux` | Linux | `scripts/setup/linux.sh` |
| Windows (`$env:OS` = `Windows_NT`, or `uname -s` starts with `MINGW`/`MSYS`) | Windows | `scripts/setup/windows.ps1` |

## 2. Check that Claude Code is logged in

Run `claude auth status`.

- Logged in: continue.
- Not logged in: tell the user, "Type `claude` in a terminal, sign in with your Claude account, then tell me when you are done." Wait. Run the check again.

The scripts check this too and stop with a plain sentence if it is not true.

## 3. Ask the only question

Ask: **"What is your full name?"** (It goes on the meeting cards, so the notes can say which lines are yours.)
Do not ask anything else. The defaults are right for everyone.

## 4. Tell the user what will happen (2 lines)

> I will install the Meeting Notes widget and a speech-to-text engine that runs only on your computer.
> It downloads about 1 GB and takes 5 to 10 minutes. You do not need to do anything until I say so.

## 5. Run the script

Use the full name from step 3 in place of `Full Name`.

- macOS: `bash scripts/setup/mac.sh --name "Full Name"`
- Linux: `bash scripts/setup/linux.sh --name "Full Name"`
- Windows: `powershell -ExecutionPolicy Bypass -File scripts\setup\windows.ps1 -Name "Full Name"`

Notes:

- It takes 5 to 10 minutes. Your shell tool may stop a command after 10 minutes: run it in the background and poll its output. If it was cut off, run it again; it is safe to repeat (it updates in place and never deletes notes or the page address).
- The output has steps like `[4/8] Installing whisper ...`. Tell the user the step name in a few words when it changes. Do not paste the output.
- Options: `--model small|turbo` (default: turbo on an Apple Silicon Mac, small everywhere else; leave it alone), `--skip-page`, `--skip-launch`, `--release-base <url-or-folder>`. On Windows they are `-Model`, `-SkipPage`, `-SkipLaunch`, `-ReleaseBase`.
- If it prints `ERROR: ...`, the sentence says what to do. Find that sentence in the troubleshooting list below, do what it says, and run the script again.

## 6. Read SETUP_RESULT

The last line of output is `SETUP_RESULT {json}`.

```json
{"ok":true,"os":"linux","config":"...","artifact_url_file":"...","mn":"...","app":"...",
 "whisper_cli":"...","whisper_model":"...","notes_dir":"...","claude_cli":"...",
 "page_url":"https://claude.ai/...","launched":true,"warnings":[]}
```

- `"ok":false` has an `error` sentence. Go to troubleshooting.
- `warnings` is a list of sentences. Handle each one with the troubleshooting list (for example a missing `pactl` or a failed `init-page`). Tell the user only what they must do.
- `launched` is `false` after `--skip-launch` or a failure. Start the app yourself: macOS `open "<app>"`, Linux `"<app>"` in the background, Windows `Start-Process "<app>"`.
- `page_url` is `null` if the page was skipped or failed.

## 7. Open the page for the user

Open `page_url` in their browser: macOS `open "<url>"`, Linux `xdg-open "<url>"`, Windows `Start-Process "<url>"`.
Tell them: "This is your private My Meetings page. It is empty now. Your notes appear here after each call. Keep it open."
If they are not signed in to claude.ai in that browser, they sign in once.

## 8. Walk through permissions and the 1-minute test

Give one instruction at a time and wait for the user to confirm before the next.

1. **Headphones.** "Put headphones on. With speakers, your microphone records the other side twice."
2. **Find the widget.** "Move the mouse to the right edge of the screen. A small tab appears. It has three buttons: Play, Stop, and Open My Meetings. `Ctrl+Alt+M` shows or hides it (Mac: `Control+Option+M`). `Ctrl+Alt+R` starts or stops a recording (Mac: `Control+Option+R`)."
3. **Press Play.**
   - **Mac:** two windows appear on the first recording: Microphone and "System Audio Recording". Tell the user to click **Allow** on both. If they clicked Don't Allow or missed it, see "Mic permission denied" below.
   - **Windows:** nothing to allow. If recording stays silent, see "Mic permission denied" below.
   - **Linux:** nothing to allow.
4. **Say out loud:** "This is a test, I will send the report tomorrow."
5. **Press Stop.** The tab shows a spinner: "Transcribing", "Writing your notes", "Updating My Meetings". A green dot means it is done. This takes about 2 minutes.
6. **Open My Meetings** (the third button, or the page you opened in step 7). Reload it if needed. "You should see a new card with a to-do: send the report, due tomorrow."

If the card appears: setup is finished. Tell the user: "From now on, press Play when a call starts and Stop when it ends. The widget starts by itself when you log in."
If no card appears after 5 minutes, use troubleshooting ("No card after the test").

## Troubleshooting

| Symptom | What to do |
|---|---|
| **No Homebrew** (`ERROR: Homebrew is not installed ...`, macOS) | Tell the user: "Please install Homebrew from https://brew.sh. It asks for your Mac password, so I cannot do it for you. Tell me when it is done." Then run the script again. |
| **Claude Code not installed / not logged in** (`ERROR: Claude Code is not ...`) | Not installed: tell the user to install it from https://claude.com/claude-code. Not logged in: tell them to type `claude` in a terminal and sign in. Then run the script again. |
| **Mac: "damaged" or "unidentified developer" when opening the app** | The app is unsigned. Run `xattr -dr com.apple.quarantine "/Applications/Meeting Notes.app"` (or `~/Applications/Meeting Notes.app` if that is where it went; `app` in SETUP_RESULT has the path), then `open` it. Otherwise: right-click the app, choose **Open**, then **Open** again. Or System Settings > Privacy & Security > scroll down > **Open Anyway**. |
| **Windows: blue "Windows protected your PC" (SmartScreen)** | Tell the user to click **More info**, then **Run anyway**. Then run the script again if it stopped. |
| **Windows: "running scripts is disabled"** | Run it exactly as in step 5, with `-ExecutionPolicy Bypass`. |
| **Linux: missing `pactl` / `parec`** (warning "Missing audio tools") | Install the package, then run the script again: Ubuntu/Debian `sudo apt install pulseaudio-utils`; Fedora `sudo dnf install pipewire-pulseaudio pulseaudio-utils`; Arch `sudo pacman -S pipewire-pulse`. Needs the user's password for `sudo`. |
| **Linux: widget does not start** (warning "libfuse2 ...") | Ubuntu/Debian `sudo apt install libfuse2`; Arch `sudo pacman -S fuse2`; then start `~/.local/bin/MeetingNotes.AppImage`. |
| **Mic permission denied / the recording is silent** | **Mac:** System Settings > Privacy & Security > **Microphone**, turn on **Meeting Notes**. Then the same under **Screen & System Audio Recording**. Quit the widget (menu bar icon > Quit) and open it again. **Windows:** Settings > Privacy & security > Microphone: turn on "Microphone access" and "Let desktop apps access your microphone". **Linux:** check the microphone is not muted in the system sound settings. Then repeat step 8 from item 3. |
| **No sound from the other side** (only "Me" lines in the notes) | **Mac:** needs macOS 14.2 or newer (Apple menu > About This Mac). On an older Mac only the microphone is recorded: tell the user to update macOS. On 14.2+ turn on **Screen & System Audio Recording** for Meeting Notes (see above) and reopen the widget. **Any OS:** the call audio must play on the computer's default output; with a Bluetooth headset, make it the default output before pressing Play. |
| **`mn init-page` failed** (warning "mn init-page failed ...", `page_url` is `null`) | Run `<mn> init-page` again (`mn` path is in SETUP_RESULT), wait for it, and read its last output line. A line `{"state":"published","url":...}` means success: use that URL. A line with `"state":"error"` (or starting with `error:`) is the reason: show **that one line** to the user. Common causes: Claude Code not logged in (sign in, run again), no internet, old Claude Code (run `claude update`). Recording and notes still work; the page is created the next time `mn init-page` or the setup script succeeds. |
| **Widget not visible** | Press `Ctrl+Alt+M` (Mac `Control+Option+M`). Look at the right screen edge and at the tray / menu bar icon: **Show**. If it is not running, start the `app` path from SETUP_RESULT. |
| **No card after the test** | Open the notes folder (`notes_dir`) and find the newest folder. Run `<mn> process "<that folder>"` and read the last line. A `"state":"error"` line says why (usually Claude Code login). Then `<mn> publish`. |

## Uninstall

Tell the user their notes in the notes folder (`~/MeetingNotes` by default) are **not** deleted unless they ask. The My Meetings page stays in their claude.ai account; they can delete it there.

First turn off start-at-login: tray / menu bar icon > uncheck **Start at login**. Then quit the widget (**Quit**) and:

**macOS**
```
rm -rf "/Applications/Meeting Notes.app" "$HOME/Applications/Meeting Notes.app" "$HOME/.local/bin/mn"
rm -rf "$HOME/Library/Application Support/meeting-notes"   # config, page address, speech model
rm -f "$HOME"/Library/LaunchAgents/*[Mm]eeting*[Nn]otes*.plist
brew uninstall whisper-cpp   # optional
```

**Linux**
```
rm -f "$HOME/.local/bin/MeetingNotes.AppImage" "$HOME/.local/bin/mn"
rm -rf "$HOME/.config/meeting-notes" "$HOME/.local/share/meeting-notes"
rm -f "$HOME"/.config/autostart/*[Mm]eeting*[Nn]otes*.desktop
```

**Windows (PowerShell)**
```
# Uninstall the app: Settings > Apps > Meeting Notes > Uninstall, or run its uninstaller silently:
Get-ChildItem "$env:LOCALAPPDATA" -Recurse -Filter "uninstall*.exe" -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match 'meeting' }   # then run it with /S
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\MeetingNotes", "$env:APPDATA\meeting-notes"
# Remove "$env:LOCALAPPDATA\MeetingNotes\bin" from the user PATH: Settings > System > About > Advanced system settings > Environment Variables.
```

Deleting the `meeting-notes` config folder also deletes `artifact_url.txt`. The page itself is not deleted.
