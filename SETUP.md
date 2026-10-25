# SETUP: runbook for the AI agent

The user opened the Claude Desktop app, went to the Code tab and pasted something like "Install Meeting Notes from https://github.com/MustafaAFarag/Meeting-Minutes by following its SETUP.md".
The user is a non-technical project manager on a Mac or a Windows PC. Everything they do is click (Authorize, Allow, Install). You run every command yourself. Ask only the questions called for in step 5.
Speak plainly: no jargon, no stack traces. Show an error line only when this file says to.
The Code tab may be open in any folder. The files go to one fixed place: `Meeting-Minutes` in the home folder (`<dir>` below).

**Command rules (they make permission prompts rare):** run one command at a time. No `&&`, no `|`, no `;`. Use `git -C <dir> ...` instead of `cd <dir>` first. Write `<dir>` as the real absolute path (for example `/Users/sarah/Meeting-Minutes` or `C:\Users\Sarah\Meeting-Minutes`), never `~` or `$HOME`.

**Done means:** the widget is running, `SETUP_RESULT` said `"ok":true`, `mn doctor` has no FAIL line, the My Meetings page opened, and the 1-minute test in step 10 produced a card.
**Stop and report** if a step fails twice in the same way. Say what failed and what the user can do.

## 0. Get git and the files

Tell the user once, now: **"If Claude asks for permission to run something, click Always allow (or Allow)."**

Find the home folder: macOS / Linux `printenv HOME`, Windows `$env:USERPROFILE`. `<dir>` is that folder plus `Meeting-Minutes`.

**Get git**

- **macOS:** run `xcode-select -p`. If it prints a path, git is there; go on. If it fails, run `xcode-select --install` and tell the user: **"A window asks to install developer tools. Click Install, then Agree, and tell me when it says it is done (5-10 minutes)."** Wait for them. Then run `git --version`; it must print a version. (Do not run `git` before this check: on a fresh Mac it opens Apple's install window by itself.)
- **Windows (PowerShell):** run `git --version`. If it fails, run `winget install --id Git.Git -e --silent --accept-package-agreements --accept-source-agreements`. This session's PATH is not refreshed, so run git by its full path from now on: `& "$env:ProgramFiles\Git\cmd\git.exe"` in place of `git` in every command below (a new session finds plain `git`). If `winget` is missing, tell the user: "Install Git from https://git-scm.com/download/win with the default options (click Next all the way), then tell me when it is done." Wait, then use the full path as above.
- **Linux:** `git` comes from the system package manager.

**Get the files** (always to the fixed folder; later updates look for it there)

- Folder `<dir>` does not exist:
  - macOS / Linux: `git clone https://github.com/MustafaAFarag/Meeting-Minutes "$HOME/Meeting-Minutes"`
  - Windows: `git clone https://github.com/MustafaAFarag/Meeting-Minutes "$HOME\Meeting-Minutes"`
- Folder exists **without** a `.git` folder (an old zip install): run these three, one at a time:
  1. `git -C "<dir>" init`
  2. `git -C "<dir>" remote add origin https://github.com/MustafaAFarag/Meeting-Minutes`
  3. `git -C "<dir>" fetch --tags --force origin`
- Folder exists **with** `.git`: `git -C "<dir>" fetch --tags --force origin`

**Use the newest release, never `main`.** Get its tag:

`git -C "<dir>" for-each-ref --sort=-v:refname --count=1 --format="%(refname:short)" "refs/tags/v*"`

It prints something like `v0.1.6`. Then: `git -C "<dir>" checkout -f v0.1.6` (use the tag you got). If no tag came back, stop and report: "No release found. Ask the developer."

## 1. Detect the OS

| Shell says | OS | Script |
|---|---|---|
| `uname -s` = `Darwin` | macOS | `<dir>/scripts/setup/mac.sh` |
| `uname -s` = `Linux` | Linux (developers only) | `<dir>/scripts/setup/linux.sh` |
| Windows (`$env:OS` = `Windows_NT`, or `uname -s` starts with `MINGW`/`MSYS`) | Windows | `<dir>\scripts\setup\windows.ps1` |

## 2. Install and sign in to Claude Code (for the widget)

**Why:** the widget writes the meeting notes by calling the standalone `claude` program in the background. The Claude Desktop app's sign-in is separate and is not shared with it, so this program must be installed and signed in on its own. Do not skip this even though the user is already signed in to Claude Desktop.

1. Find `claude`: macOS / Linux `<home>/.local/bin/claude`, Windows `<home>\.local\bin\claude.exe` (or on PATH). Call that path `<claude>`.
2. Not found, install it with the official installer:
   - macOS: `curl -fsSL https://claude.ai/install.sh -o "$TMPDIR/claude-install.sh"`, then `bash "$TMPDIR/claude-install.sh"`. (If `$TMPDIR` is empty, use `/tmp`.)
   - Windows: `Invoke-RestMethod https://claude.ai/install.ps1 -OutFile "$env:TEMP\claude-install.ps1"`, then `powershell -ExecutionPolicy Bypass -File "$env:TEMP\claude-install.ps1"`.
   The setup script in step 7 also does this if it is still missing.
3. Run `<claude> auth status`. If it says logged in, go on to step 3.
4. Not logged in: tell the user **"A browser tab will open. Click Authorize with your CTCHealth Claude account, then come back here."** Run `<claude> auth login`. It waits for the browser: run it in the background and poll its output until it finishes (give it up to 3 minutes). Then run `<claude> auth status` again.
5. Still not logged in, or `auth login` could not run from here: tell the user **"Open Terminal (Windows: PowerShell), type `claude`, press Enter, sign in with your CTCHealth Claude account, then type `/exit` and press Enter. Tell me when you are done."** Wait, then run `<claude> auth status` again.

## 3. Pre-approve the update commands

So that later updates run without permission prompts from any folder, add these rules to the user-level Claude Code settings file `<home>/.claude/settings.json` (Windows `<home>\.claude\settings.json`), under `permissions` > `allow`.

- If the file does not exist, create it with `{"permissions":{"allow":[...]}}`. If it exists, read it, **keep everything already in it** (other keys, other rules), add only the rules below that are missing, and write it back as valid JSON. If it cannot be parsed, do not touch it: skip this step and tell the user nothing (updates will just ask for permission).
- Use the **real absolute paths**: expand `<home>` and `<dir>` yourself. Write each rule exactly as shown: it is the whole command, not a prefix, so do not add ` *` or anything else (the only `*` are the ones shown). Quote paths the same way as in the commands (inside JSON write each `"` as `\"`; on Windows write each `\` as `\\`).
- Use the rule type of the tool you run commands with (`Bash(...)`; if your shell tool is PowerShell, `PowerShell(...)`).
- If the file already has an older Meeting Notes rule that ends in ` *` (for example `Bash(git -C "<dir>" *)` or `Bash(bash "<dir>/scripts/setup/mac.sh" *)`), remove it: it allowed more than these commands.

**macOS** (for Linux use `linux.sh` in place of `mac.sh`):

```
Bash(git -C "<dir>" rev-parse --show-toplevel)
Bash(git -C "<dir>" describe --tags --exact-match)
Bash(git -C "<dir>" fetch --tags --force origin)
Bash(git -C "<dir>" for-each-ref --sort=-v:refname --count=1 --format="%(refname:short)" "refs/tags/v*")
Bash(git -C "<dir>" checkout -f v*)
Bash(bash "<dir>/scripts/setup/mac.sh" --update)
Bash("<home>/.local/bin/mn" doctor)
Bash("<home>/.local/bin/mn" init-page)
Bash("<home>/.local/bin/claude" auth status)
```

**Windows** (PowerShell commands):

```
PowerShell(git -C "<dir>" rev-parse --show-toplevel)
PowerShell(git -C "<dir>" describe --tags --exact-match)
PowerShell(git -C "<dir>" fetch --tags --force origin)
PowerShell(git -C "<dir>" for-each-ref --sort=-v:refname --count=1 --format="%(refname:short)" "refs/tags/v*")
PowerShell(git -C "<dir>" checkout -f v*)
PowerShell(powershell -ExecutionPolicy Bypass -File "<dir>\scripts\setup\windows.ps1" -Update)
PowerShell(& "<home>\AppData\Local\MeetingNotes\bin\mn.exe" doctor)
PowerShell(& "<home>\AppData\Local\MeetingNotes\bin\mn.exe" init-page)
PowerShell(& "<home>\.local\bin\claude.exe" auth status)
```

(If your shell tool for Windows is Bash, write `Bash(...)` instead of `PowerShell(...)`.) These rules cover only the exact update commands. Do not add anything broader (no `Bash(*)`, no ` *` at the end, no `curl`, no `rm`).

## 4. Check the Microsoft 365 connector

Run `<claude> mcp list` (it can take up to a minute) and find the line that starts with `claude.ai Microsoft 365:`.
Real output on a connected CTCHealth account:

`claude.ai Microsoft 365: https://microsoft365.mcp.claude.com/mcp - ✔ Connected`

Other states look like `- ! Needs authentication`.

- **Connected:** say nothing and continue.
- **Needs authentication:** tell the user: "Open claude.ai, go to Settings > Connectors, find Microsoft 365 and click Connect, then sign in with your work Microsoft account. Tell me when it is done." Wait, then run `<claude> mcp list` again. If it still says `Needs authentication`, continue anyway and tell them: "Meeting names will come from the conversation instead of Outlook until Microsoft 365 is connected."
- **No Microsoft 365 line:** ask once: "Is this the CTCHealth Claude account you signed in with a moment ago?" If not, run `<claude> auth logout`, then repeat step 2 from item 4 with the right account and check this step again. If they say it is, continue; the notes still work without Microsoft 365. If the line is still missing after they sign in again, continue anyway and tell them meeting names will come from the conversation instead of Outlook until it is connected.

Never block setup on this check.

## 5. Ask for their name

Run the matching command and read its `DETECTED_NAME` line:

- macOS: `bash "<dir>/scripts/setup/mac.sh" --detect-name`
- Linux: `bash "<dir>/scripts/setup/linux.sh" --detect-name`
- Windows: `powershell -ExecutionPolicy Bypass -File "<dir>\scripts\setup\windows.ps1" -DetectName`

If a name came back, ask: **"Is your name Sarah Smith? Reply yes, or type your full name."** Replace `Sarah Smith` with the detected name. Ask the same way when the source is `claude-email-guess`.
If no name came back, ask: **"What is your full name?"**
If they reply yes, use the detected name; otherwise, use the name they type.
It goes on the meeting cards, so the notes know which to-dos are yours.
Do not ask anything else. The defaults are right for everyone.

## 6. Tell the user what will happen (2 lines)

> I will install the Meeting Notes widget and a speech-to-text engine that runs only on your computer.
> It downloads about 1 GB and takes 5 to 10 minutes. You do not need to do anything until I say so.

## 7. Run the script

Use the full name from step 5 in place of `Full Name`, and the absolute `<dir>`.

- macOS: `bash "<dir>/scripts/setup/mac.sh" --name "Full Name"`
- Linux: `bash "<dir>/scripts/setup/linux.sh" --name "Full Name"`
- Windows: `powershell -ExecutionPolicy Bypass -File "<dir>\scripts\setup\windows.ps1" -Name "Full Name"`

Notes:

- It takes 5 to 10 minutes. Your shell tool may stop a command after 10 minutes: run it in the background and poll its output. If it was cut off, run it again; it is safe to repeat (it updates in place and never deletes notes or the page address).
- The output has steps like `[4/8] Installing whisper ...`. Tell the user the step name in a few words when it changes. Do not paste the output.
- Options: `--model small|turbo` (default: turbo on an Apple Silicon Mac, small everywhere else; leave it alone), `--skip-page`, `--skip-launch`, `--release-base <url-or-folder>` (leave it alone: the script takes the app and `mn` from the release of the tag checked out in step 0, and prints `Using release v0.1.6`). On Windows they are `-Model`, `-SkipPage`, `-SkipLaunch`, `-ReleaseBase`.
- If it says "This version is still being prepared. Wait 30 minutes, then run this script again.": the release for this tag is still being built. Tell the user "The newest version is not quite ready. I will try again in 30 minutes." and run the script again after that (or check out the previous tag, step 0, if the user cannot wait).
- If it prints `ERROR: ...`, the sentence says what to do. Find that sentence in the troubleshooting list below, do what it says, and run the script again.

## 8. Read SETUP_RESULT, then run the health check

The last line of output is `SETUP_RESULT {json}`.

```json
{"ok":true,"os":"linux","config":"...","artifact_url_file":"...","mn":"...","app":"...",
 "whisper_cli":"...","whisper_model":"...","notes_dir":"...","claude_cli":"...",
 "page_url":"https://claude.ai/...","launched":true,"update":false,"release":"v0.1.6","warnings":[]}
```

- `"ok":false` has an `error` sentence. Go to troubleshooting.
- `warnings` is a list of sentences. Handle each one with the troubleshooting list (for example a missing `pactl` or a failed `init-page`). Tell the user only what they must do.
- `launched` is `false` after `--skip-launch` or a failure. Start the app yourself: macOS `open "<app>"`, Linux `"<app>"` in the background, Windows `Start-Process "<app>"`.
- `page_url` is `null` if the page was skipped or failed.

Then run the health check with the `mn` path from SETUP_RESULT: `<mn> doctor` (Windows: `& "<mn>" doctor`). It prints one line per check (`OK`, `WARN` or `FAIL` and a sentence) and ends with `DOCTOR_RESULT {json}`.

- `"ok":true`: continue.
- A `FAIL` line: fix it with "`mn doctor` FAIL lines" in the troubleshooting list, then run `doctor` again. Do not go on while a FAIL is open.
- A `WARN` line: tell the user only if they must do something; otherwise ignore it.

## 9. Open the page for the user

Open `page_url` in their browser: macOS `open "<url>"`, Linux `xdg-open "<url>"`, Windows `Start-Process "<url>"`.
Tell them: "This is your private My Meetings page. It is empty now. Your notes appear here after each call. Keep it open."
If they are not signed in to claude.ai in that browser, they sign in once.

## 10. Walk through permissions and the 1-minute test

Give one instruction at a time and wait for the user to confirm before the next.

1. **Headphones.** "Put headphones on. With speakers, your microphone records the other side twice."
2. **Find the widget.** "Move the mouse to the right edge of the screen. A small tab appears. It has three buttons: Play, Stop, and Open My Meetings. `Ctrl+Alt+M` shows or hides it (Mac: `Control+Option+M`). `Ctrl+Alt+R` starts or stops a recording (Mac: `Control+Option+R`)."
3. **Press Play.**
   - **Mac:** two windows appear on the first recording: Microphone and "System Audio Recording". Tell the user to click **Allow** on both. If they clicked Don't Allow or missed it, see "Mic permission denied" below.
   - **Windows:** nothing to allow. If recording stays silent, see "Mic permission denied" below.
   - **Linux:** nothing to allow.
4. **Say out loud:** "This is a test, I will send the report tomorrow."
5. **Press Stop.** The tab shows a spinner: "Transcribing", "Writing your notes", "Updating My Meetings". A green dot means it is done. This takes about 2 minutes.
6. **Open My Meetings** (the third button, or the page you opened in step 9). Reload it if needed. "You should see a new card with a to-do: send the report, due tomorrow."

If no card appears after 5 minutes, use troubleshooting ("No card after the test").

## 11. Final words to the user

If the card appeared, setup is finished. Say these things, in plain words:

- **"From now on, press Play when a call starts and Stop when it ends. The widget starts by itself when you log in."**
- **How to open it again.** macOS: "Click the Meeting Notes icon in the Dock (setup put it there), or press Command+Space and type Meeting Notes." Windows: "Click the Meeting Notes icon on the Desktop, or find Meeting Notes in the Start menu." "If it is already running, clicking the icon just shows the widget. Closing the widget only hides it; it starts again by itself when you log in."
- **Updates.** "When the widget says **Update available**, click **Update** and paste what it copies into Claude Code. Claude does the rest."

## Troubleshooting

| Symptom | What to do |
|---|---|
| **git / Xcode tools install failed** (step 0: `xcode-select --install` error, no window, or `git --version` still fails after the user said it was done) | **Mac:** the window may be hidden behind others, or the download failed. Tell the user: "Open System Settings > General > Software Update, install any 'Command Line Tools' update, and tell me when it is done." If there is none, run `xcode-select --install` once more. Still failing: tell the user to ask the developer. **Windows:** if `winget` failed or is missing, tell the user to install Git from https://git-scm.com/download/win with the default options, then use `"$env:ProgramFiles\Git\cmd\git.exe"` as in step 0. |
| **No Homebrew** (`ERROR: Could not install whisper...`, macOS) | This appears only when the whisper download failed. Tell the user: "Open Terminal, paste the command from https://brew.sh, press Enter and type your Mac password when asked. Then run this script again." |
| **Claude Code not installed / not signed in** (`ERROR: Claude Code is not signed in` or `Could not install Claude Code`) | Not signed in: do step 2 from item 3 (`<claude> auth login`, then the Terminal fallback). Could not install: check the internet connection, then run the installer command of step 2 again; if it fails twice, report the one error line. Then run the setup script again. |
| **Mac: "damaged" or "unidentified developer" when opening the app** | The app is unsigned. Run `xattr -dr com.apple.quarantine "/Applications/Meeting Notes.app"` (or `<home>/Applications/Meeting Notes.app` if that is where it went; `app` in SETUP_RESULT has the path), then `open` it. Otherwise: right-click the app, choose **Open**, then **Open** again. Or System Settings > Privacy & Security > scroll down > **Open Anyway**. |
| **Windows: blue "Windows protected your PC" (SmartScreen)** | Tell the user to click **More info**, then **Run anyway**. Then run the script again if it stopped. |
| **Windows: "running scripts is disabled"** | Run it exactly as in step 7, with `-ExecutionPolicy Bypass`. |
| **Linux: missing `pactl` / `parec`** (warning "Missing audio tools") | Install the package, then run the script again: Ubuntu/Debian `sudo apt install pulseaudio-utils`; Fedora `sudo dnf install pipewire-pulseaudio pulseaudio-utils`; Arch `sudo pacman -S pipewire-pulse`. Needs the user's password for `sudo`. |
| **Linux: widget does not start** (warning "libfuse2 ...") | Ubuntu/Debian `sudo apt install libfuse2`; Arch `sudo pacman -S fuse2`; then start `~/.local/bin/MeetingNotes.AppImage`. |
| **Mic permission denied / the recording is silent** | **Mac:** System Settings > Privacy & Security > **Microphone**, turn on **Meeting Notes**. Then the same under **Screen & System Audio Recording**. Quit the widget (menu bar icon > Quit) and open it again. **Windows:** Settings > Privacy & security > Microphone: turn on "Microphone access" and "Let desktop apps access your microphone". **Linux:** check the microphone is not muted in the system sound settings. Then repeat step 10 from item 3. |
| **No sound from the other side** (only "Me" lines in the notes) | **Mac:** needs macOS 14.2 or newer (Apple menu > About This Mac). On an older Mac only the microphone is recorded: tell the user to update macOS. On 14.2+ turn on **Screen & System Audio Recording** for Meeting Notes (see above) and reopen the widget. **Any OS:** the call audio must play on the computer's default output; with a Bluetooth headset, make it the default output before pressing Play. |
| **`mn init-page` failed** (warning "mn init-page failed ...", `page_url` is `null`) | Run `<mn> init-page` again (`mn` path is in SETUP_RESULT), wait for it, and read its last output line. A line `{"state":"published","url":...}` means success: use that URL. A line with `"state":"error"` (or starting with `error:`) is the reason: show **that one line** to the user. Common causes: Claude Code not signed in (do step 2, run again), no internet, old Claude Code (run `<claude> update`). Recording and notes still work; the page is created the next time `mn init-page` or the setup script succeeds. |
| **Widget not visible** | Press `Ctrl+Alt+M` (Mac `Control+Option+M`). Look at the right screen edge and at the tray / menu bar icon: **Show**. Or click the Meeting Notes icon (Dock, Desktop or Start menu). If it is not running, start the `app` path from SETUP_RESULT. |
| **No card after the test** | Open the notes folder (`notes_dir`) and find the newest folder. Run `<mn> process "<that folder>"` and read the last line. A `"state":"error"` line says why (usually Claude Code sign-in: do step 2). Then `<mn> publish`. |
| **`mn doctor` FAIL lines** | Each FAIL has a check `name` in the `checks` list of the `DOCTOR_RESULT` line. `config`: the settings file is missing or broken: run the setup script again (step 7). `version`: the installed `mn` is not the release you checked out: run the setup script again, then check `<dir>` is on the newest tag (step 0). `whisper` or `whisper_run`: the speech engine is missing or does not start: run the setup script again; on Windows, if `whisper_run` still fails, install the "Microsoft Visual C++ Redistributable" from microsoft.com. `claude`: Claude Code is missing or not signed in: do step 2. `vad` (only ever a WARN): the speech-detection file is missing: run the setup script again. `notes`: the notes folder cannot be written: check the folder in `notes_dir` (SETUP_RESULT) exists and is not read-only (macOS: it may be in a folder the user denied access to); create it with `mkdir`/`New-Item` if missing. `page`: the My Meetings page address is missing: run `<mn> init-page` (see the `mn init-page` row). Any other name, or still failing after one fix: run the setup script again, then `doctor` again; if it fails the same way twice, stop and report the one FAIL line. |

## Uninstall

Tell the user their notes in the notes folder (`~/MeetingNotes` by default) are **not** deleted unless they ask. The My Meetings page stays in their claude.ai account; they can delete it there.

First turn off start-at-login: tray / menu bar icon > uncheck **Start at login**. Then quit the widget (**Quit**) and:

**macOS**
```
rm -rf "/Applications/Meeting Notes.app" "$HOME/Applications/Meeting Notes.app" "$HOME/.local/bin/mn"
rm -rf "$HOME/Library/Application Support/meeting-notes"   # config, page address, speech model
rm -f "$HOME"/Library/LaunchAgents/*[Mm]eeting*[Nn]otes*.plist
rm -rf "$HOME/Meeting-Minutes"   # these setup files
brew uninstall whisper-cpp   # optional
```
Remove the Dock icon: drag **Meeting Notes** out of the Dock and let go (or right-click it > Options > Remove from Dock).

**Linux**
```
rm -f "$HOME/.local/bin/MeetingNotes.AppImage" "$HOME/.local/bin/mn"
rm -rf "$HOME/.config/meeting-notes" "$HOME/.local/share/meeting-notes"
rm -f "$HOME"/.config/autostart/*[Mm]eeting*[Nn]otes*.desktop "$HOME/.local/share/applications/meeting-notes.desktop"
rm -rf "$HOME/Meeting-Minutes"
```

**Windows (PowerShell)**
```
# Uninstall the app: Settings > Apps > Meeting Notes > Uninstall, or run its uninstaller silently:
Get-ChildItem "$env:LOCALAPPDATA" -Recurse -Filter "uninstall*.exe" -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match 'meeting' }   # then run it with /S
Remove-Item -Recurse -Force "$env:LOCALAPPDATA\MeetingNotes", "$env:APPDATA\meeting-notes"
Remove-Item -Force "$([Environment]::GetFolderPath('Desktop'))\Meeting Notes.lnk"   # the Desktop icon
Remove-Item -Recurse -Force "$HOME\Meeting-Minutes"   # these setup files
# Remove "$env:LOCALAPPDATA\MeetingNotes\bin" from the user PATH: Settings > System > About > Advanced system settings > Environment Variables.
```

Deleting the `meeting-notes` config folder also deletes `artifact_url.txt`. The page itself is not deleted.
