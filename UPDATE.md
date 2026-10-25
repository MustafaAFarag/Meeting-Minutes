# UPDATE: runbook for the AI agent

## Automatic update

The widget's **Update** button updates the app by itself, with no Claude Code: it runs `scripts/setup/update.sh` (macOS, Linux) or `scripts/setup/update.ps1` (Windows). Those scripts do steps 2 to 7 below (fetch the tags, check out the newest one, run the setup script with `--update`, run `mn doctor`) and put the old version back if something fails. This runbook is the **repair path** for when that fails: the app's "Get help" button copies the sentence that points here.

When you are here to repair a failed update, read these two files first (they are in the settings folder: macOS `<home>/Library/Application Support/meeting-notes`, Linux `<home>/.config/meeting-notes`, Windows `<home>\AppData\Roaming\meeting-notes`):

- `update-status.json`: `state` (`running`, `up_to_date`, `done` or `failed`), `from`, `to`, `error` (one plain-English line) and `log`. A `running` state that does not change for 15 minutes means the update was interrupted: treat it as failed.
- `update.log`: everything the updater and the setup script printed, with times. It is rewritten on every run.

Use the `error` line and the end of `update.log` to see which step failed, then follow the steps below from the start (they are safe to repeat). `error` says "Git is missing" or "install folder is damaged": the files were not installed the normal way, so redo the install with `SETUP.md` (step 1 below).

## For the AI agent

The user clicked **Update** on the Meeting Notes widget and pasted this into the Claude Desktop app (Code tab): "Update Meeting Notes to the latest version by following the steps in `<home>/Meeting-Minutes/UPDATE.md`".
The user is a non-technical project manager on a Mac or a Windows PC. Everything they do is click. You run every command yourself. Ask them nothing.
Speak plainly: no jargon, no stack traces. Never paste logs to the user.
The Code tab may be open in any folder. The files are in `Meeting-Minutes` in the home folder: `<dir>` below, as an absolute path. The path in the sentence the user pasted tells you the home folder (for example `/Users/sarah/Meeting-Minutes` or `C:\Users\Sarah\Meeting-Minutes`).

**Command rules (they keep permission prompts rare):** run one command at a time. No `&&`, no `|`, no `;`. Use `git -C "<dir>" ...` instead of `cd <dir>` first. Write `<dir>` as the real absolute path in every command, never `~` or `$HOME`. If Claude asks for permission, the user clicks **Always allow**.

**Done means:** the folder is on the newest release tag, the setup script said `"ok":true`, `mn doctor` has no FAIL line and shows the new version, and the user was told what is new.
**Stop and report** if something fails twice in the same way: go to "If it fails" at the end.

On Windows run the commands in PowerShell. If `git` is not found there, use `& "$env:ProgramFiles\Git\cmd\git.exe"` in its place.

## 1. Find the files

`git -C "<dir>" rev-parse --show-toplevel`

It must print `<dir>` itself (Windows may print it with `/` instead of `\`: that is the same folder).

- It fails, or prints a different folder (`<dir>` is missing, or has no `.git` of its own): the files were not installed the normal way. Tell the user "I need to redo the install. It keeps your notes and your My Meetings page." Then follow `SETUP.md` from step 0 (if `<dir>` has no `SETUP.md`, the web page https://github.com/MustafaAFarag/Meeting-Minutes/blob/main/SETUP.md has the same steps). Do not go on with this file.
- Present: continue.

## 2. Remember the current version

`git -C "<dir>" describe --tags --exact-match`

It prints a tag like `v0.1.5`. Note it as the **old tag**. If it fails (an old checkout of `main`), note the old tag as "unknown".

## 3. Get the latest releases

`git -C "<dir>" fetch --tags --force origin`

`--force` is needed because a tag can be re-created.

## 4. Find the newest release

`git -C "<dir>" for-each-ref --sort=-v:refname --count=1 --format="%(refname:short)" "refs/tags/v*"`

It prints the **newest tag**, for example `v0.1.6`. It must be `v` and three numbers with dots; anything else (like `vtest` or `v1.0.0-rc1`) is not a release: stop and report "No release found. Ask the developer." If nothing is printed, go to "If it fails" (reason: no release found).

If the newest tag **equals the old tag**: tell the user **"You already have the newest version (v0.1.6)."** (with the real tag). Still run step 7 (`mn doctor`) and tell them the result in one line; if a FAIL line shows, fix it with `SETUP.md` > Troubleshooting > "`mn doctor` FAIL lines". Then stop.

## 5. Switch to the newest release

`git -C "<dir>" checkout -f v0.1.6` (use the newest tag).

This also works after the history was rewritten. Local changes in this folder are not expected; they are discarded.

## 6. Run the update

Tell the user: **"Updating Meeting Notes. This takes a few minutes. The widget will close and open again by itself. Your notes and your page are not touched."**

- macOS: `bash "<dir>/scripts/setup/mac.sh" --update`
- Windows: `powershell -ExecutionPolicy Bypass -File "<dir>\scripts\setup\windows.ps1" -Update`
- Linux (developers): `bash "<dir>/scripts/setup/linux.sh" --update`

Notes (same as `SETUP.md` step 7):

- Your shell tool may stop a command after 10 minutes: run it in the background and poll its output. If it was cut off, run it again; it is safe to repeat.
- The output has steps like `[4/8] ...`. Tell the user the step name in a few words when it changes. Do not paste the output.
- It keeps the name, notes, My Meetings page and settings. It replaces the app and `mn`, refreshes the page and starts the widget again.
- The last line is `SETUP_RESULT {json}`. `"ok":true` and `"update":true` is success. `warnings` is a list of sentences: handle each with `SETUP.md` > Troubleshooting; tell the user only what they must do. If `launched` is `false`, start the `app` path yourself (macOS `open "<app>"`, Windows `Start-Process "<app>"`).
- `"ok":false` has an `error` sentence. Find it in `SETUP.md` > Troubleshooting, do what it says, and run the script once more. If it fails a second time, go to "If it fails".
- The script takes the app and `mn` from the release of the tag you checked out, and prints `Using release v0.1.6`. If it says **"This version is still being prepared"**, the release is still being built (15 to 25 minutes after it is published) and nothing was replaced. Do not retry. If the old tag is a real tag, run `git -C "<dir>" checkout -f <old tag>` so the folder matches what is installed. Tell the user **"The newest version is not quite ready. Please click Update again in about 30 minutes."** and stop. This is not a failure: do not write the developer message.
- The script says Claude Code is not signed in: do `SETUP.md` step 2, then run the script again.

## 7. Health check

Run `mn doctor` with the `mn` path from SETUP_RESULT (macOS / Linux `<home>/.local/bin/mn`, Windows `& "<home>\AppData\Local\MeetingNotes\bin\mn.exe"`): `<mn> doctor`.

- The last line is `DOCTOR_RESULT {json}`. Its `version` must equal the newest tag without the `v` (`v0.1.6` is `0.1.6`). If not, run step 6 once more, then check again.
- A `FAIL` line: fix it with `SETUP.md` > Troubleshooting > "`mn doctor` FAIL lines", then run `doctor` again. If it cannot be fixed, go to "If it fails".
- A `WARN` line: tell the user only if they must do something.

## 7b. Refresh the update rules

Do `SETUP.md` step 3 again, quietly: it replaces older, wider rules in `<home>/.claude/settings.json` with the exact update commands. Say nothing to the user about it.

## 8. Tell the user what is new

Read `<dir>/CHANGELOG.md`. Take the "What's new for you" bullets of the newest version, and of every version between the old tag and the newest one if the user skipped some (old tag "unknown": newest version only). Tell the user **at most 3 short bullets**, in plain words, with no technical words. Then add: "If anything looks wrong, tell me." The update is finished.

## If it fails

Use this when setup gives `"ok":false` twice, or `mn doctor` has a FAIL you cannot fix, or no release was found.

1. If the old tag is a real tag: `git -C "<dir>" checkout -f <old tag>`, run step 6 again, then `mn doctor`. (Old tag "unknown": skip this item.)
2. Tell the user: **"The update did not work, so I put the old version back. Your notes are safe. Please send this message to Mustafa:"** (say what actually happened: if going back also failed, say so instead of "put the old version back"). Then give them a short, ready-to-send message in this shape, filled in:

> Meeting Notes update failed.
> Computer: Mac / Windows (version)
> Old version: v0.1.5. New version: v0.1.6.
> Failed step: 6 (run the update).
> Error: <the one error line>

Use exactly one error line. Never paste logs or long output to the user.
