#!/usr/bin/env bash
# Meeting Notes one-click updater for macOS and Linux. The widget's Update button runs a COPY of this
# file (checking out a new tag rewrites the original), detached from the app. It never prompts.
#
#   bash update.sh --repo <abs repo dir> --status <abs status file>
#
# It writes the status file (state running, then up_to_date, done or failed) and update.log next to it.
# A failure always ends in state "failed", never a status stuck on "running".
set -Eeuo pipefail

REPO=""
STATUS=""

die_usage() {
  printf 'ERROR: %s\nUsage: bash update.sh --repo <abs repo dir> --status <abs status file>\n' "$1" >&2
  exit 2
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo) [ "$#" -ge 2 ] || die_usage "--repo needs a value."; REPO="$2"; shift 2 ;;
    --repo=*) REPO="${1#--repo=}"; shift ;;
    --status) [ "$#" -ge 2 ] || die_usage "--status needs a value."; STATUS="$2"; shift 2 ;;
    --status=*) STATUS="${1#--status=}"; shift ;;
    *) die_usage "Unknown option $1." ;;
  esac
done
[ -n "$REPO" ] || die_usage "--repo is required."
[ -n "$STATUS" ] || die_usage "--status is required."

# The app may have been started from a mount that disappears when it quits: do not stay inside it.
cd "$HOME" 2>/dev/null || cd /
# Survive the app (and its session) going away while the setup script quits it.
trap '' HUP
trap 'exit 1' INT TERM

# An AppImage puts its own folders first on these; the system tools must not load libraries from it.
if [ -n "${APPDIR:-}" ]; then
  strip_appdir() {
    printf '%s' "$1" | tr ':' '\n' | grep -v -F -x -e '' | grep -v -F -e "$APPDIR" | paste -sd: - || true
  }
  PATH="$(strip_appdir "$PATH")"
  if [ -n "${LD_LIBRARY_PATH:-}" ]; then LD_LIBRARY_PATH="$(strip_appdir "$LD_LIBRARY_PATH")"; fi
  export PATH LD_LIBRARY_PATH
fi
# A Mac app starts with a bare PATH.
PATH="$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
export PATH
# No git prompt may ever wait for a person; a stalled download fails instead of hanging.
export GIT_TERMINAL_PROMPT=0 GCM_INTERACTIVE=never

OS="$(uname -s)"
STATUS_DIR="$(dirname "$STATUS")"
LOG="$STATUS_DIR/update.log"
MN="$HOME/.local/bin/mn"
case "$OS" in
  Darwin) SETUP_NAME="mac.sh" ;;
  Linux) SETUP_NAME="linux.sh" ;;
  *) SETUP_NAME="" ;;
esac
SETUP="$REPO/scripts/setup/$SETUP_NAME"

FROM="unknown"
TO=""
FINAL_WRITTEN=0
APP_MAY_BE_DOWN=0
WORK_DIR=""
SETUP_PID=""

mkdir -p "$STATUS_DIR" 2>/dev/null || true
: >"$LOG" 2>/dev/null || true

log() {
  printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$1" >>"$LOG" 2>/dev/null || true
}

# One line of plain text, safe inside a JSON string.
json_str() {
  printf '%s' "$1" | tr '\000-\037' ' ' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# write_status <state> <to or empty> <error or empty>: atomic (write .tmp, then rename).
write_status() {
  local state="$1" to="$2" err="$3" to_json err_json
  if [ -n "$to" ]; then to_json="\"$(json_str "$to")\""; else to_json="null"; fi
  if [ -n "$err" ]; then err_json="\"$(json_str "$err")\""; else err_json="null"; fi
  printf '{"state":"%s","from":"%s","to":%s,"error":%s,"log":"%s","updated_at":"%s","seen":false}\n' \
    "$state" "$(json_str "$FROM")" "$to_json" "$err_json" "$(json_str "$LOG")" \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"$STATUS.tmp"
  mv -f "$STATUS.tmp" "$STATUS"
}

# Start the widget again if it is not running. Best effort: never fails the script.
ensure_app_running() {
  local app_dir app
  case "$OS" in
    Darwin)
      if pgrep -f "Meeting Notes.app/Contents/MacOS" >/dev/null 2>&1; then return 0; fi
      for app_dir in "/Applications" "$HOME/Applications"; do
        app="$app_dir/Meeting Notes.app"
        if [ -d "$app" ]; then
          log "Starting the widget again: $app"
          open "$app" >/dev/null 2>&1 || true
          return 0
        fi
      done
      ;;
    Linux)
      app="$HOME/.local/bin/MeetingNotes.AppImage"
      if pgrep -f "$app" >/dev/null 2>&1; then return 0; fi
      if [ -x "$app" ] && [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
        log "Starting the widget again: $app"
        setsid nohup "$app" >/dev/null 2>&1 </dev/null &
      fi
      ;;
  esac
  return 0
}

# finish <state> <to or empty> <error or empty>: the one place a final status is written.
finish() {
  FINAL_WRITTEN=1
  write_status "$1" "$2" "$3"
  log "Finished: $1${3:+ ($3)}"
}

fail() {
  finish "failed" "$TO" "$1"
  if [ "$APP_MAY_BE_DOWN" -eq 1 ]; then ensure_app_running; fi
  exit 1
}

on_exit() {
  local code=$?
  set +e
  # Killed while setup runs: stop setup too, so nothing keeps replacing files after the status is final.
  if [ -n "$SETUP_PID" ]; then pkill -TERM -P "$SETUP_PID" 2>/dev/null; kill "$SETUP_PID" 2>/dev/null; fi
  if [ -n "$WORK_DIR" ]; then rm -rf "$WORK_DIR"; fi
  if [ "$FINAL_WRITTEN" -eq 0 ]; then
    log "The script stopped unexpectedly (exit code $code, line ${FAIL_LINE:-?})."
    write_status "failed" "$TO" "The update stopped unexpectedly. Ask Claude to repair the install."
    ensure_app_running
  fi
}
FAIL_LINE=""
trap 'FAIL_LINE=$LINENO' ERR
trap on_exit EXIT

last_line() {
  grep -v '^[[:space:]]*$' | tail -n 1 | tr -d '\r' | cut -c 1-300 || true
}

# ---------------------------------------------------------------- 1. running
if command -v git >/dev/null 2>&1; then
  GIT="$(command -v git)"
else
  GIT=""
fi
# On a Mac without the developer tools, /usr/bin/git is a stub that opens an install window.
if [ "$OS" = "Darwin" ] && [ "$GIT" = "/usr/bin/git" ] && ! xcode-select -p >/dev/null 2>&1; then GIT=""; fi

if [ -n "$GIT" ]; then
  FROM="$("$GIT" -C "$REPO" describe --tags --exact-match --match 'v[0-9]*' 2>/dev/null | head -n 1)" || FROM=""
  case "$FROM" in v[0-9]*) ;; *) FROM="unknown" ;; esac
fi
write_status "running" "" ""
log "Update started. Installed version: $FROM. Repo: $REPO"

[ -n "$GIT" ] || fail "Git is missing. Ask Claude to repair the install."
[ -n "$SETUP_NAME" ] || fail "This computer is not supported (only macOS and Linux)."
git_in_repo() { "$GIT" -C "$REPO" -c http.lowSpeedLimit=1000 -c http.lowSpeedTime=60 "$@"; }
"$GIT" -C "$REPO" rev-parse --git-dir >/dev/null 2>&1 ||
  fail "The Meeting Notes install folder is damaged. Ask Claude to repair the install."

# ---------------------------------------------------------------- 2. newest tag
GIT_OUT=""
if ! GIT_OUT="$(git_in_repo fetch --tags --force origin 2>&1)"; then
  log "git fetch failed: $GIT_OUT"
  fail "Could not check for updates: $(printf '%s\n' "$GIT_OUT" | last_line). Check the internet connection."
fi
# Only real release tags (vX.Y.Z) count: awk, not head, so for-each-ref never gets SIGPIPE under pipefail.
NEWEST="$(git_in_repo for-each-ref --sort=-v:refname --format='%(refname:short)' 'refs/tags/v*' 2>/dev/null |
  awk '!n && /^v[0-9]+\.[0-9]+\.[0-9]+$/ { print; n = 1 }')" || NEWEST=""
[ -n "$NEWEST" ] || fail "No release was found."
log "Newest release: $NEWEST"

# version_gt vA.B.C vX.Y.Z: true when the first is newer.
version_gt() {
  local a b i
  IFS=. read -r -a a <<<"${1#v}"
  IFS=. read -r -a b <<<"${2#v}"
  for i in 0 1 2; do
    [ "$((10#${a[$i]}))" -gt "$((10#${b[$i]}))" ] && return 0
    [ "$((10#${a[$i]}))" -lt "$((10#${b[$i]}))" ] && return 1
  done
  return 1
}

# ---------------------------------------------------------------- 3. up to date
# Never move backwards: a newest tag that is not newer than the installed one is "up to date".
if [[ "$FROM" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] && ! version_gt "$NEWEST" "$FROM"; then
  NEWEST="$FROM"
fi
if [ "$NEWEST" = "$FROM" ]; then
  TO="$NEWEST"
  finish "up_to_date" "$TO" ""
  exit 0
fi
TO="$NEWEST"

# ---------------------------------------------------------------- helpers for 4 to 7
# run_setup: runs the checked-out setup script with --update. Output goes to the log and to $SETUP_OUT.
# Sets SETUP_RC. Setup quits the app, so from here on a failure must start it again.
SETUP_OUT=""
SETUP_RC=0
run_setup() {
  APP_MAY_BE_DOWN=1
  SETUP_OUT="$WORK_DIR/setup.out"
  log "Running $SETUP_NAME --update"
  # In the background plus wait: a signal is then handled at once, not after setup ends.
  ( bash "$SETUP" --update </dev/null 2>&1 | tee -a "$LOG" >"$SETUP_OUT" ) &
  SETUP_PID=$!
  SETUP_RC=0
  wait "$SETUP_PID" || SETUP_RC=$?
  SETUP_PID=""
}

# setup_ok: true when the last SETUP_RESULT line of $SETUP_OUT says "ok":true.
setup_ok() {
  grep '^SETUP_RESULT ' "$SETUP_OUT" 2>/dev/null | tail -n 1 | grep -E '"ok"[[:space:]]*:[[:space:]]*true' >/dev/null 2>&1
}

# setup_error: the most useful line of a failed setup run.
setup_error() {
  local line msg
  line="$(grep '^SETUP_RESULT ' "$SETUP_OUT" 2>/dev/null | tail -n 1 || true)"
  msg="$(printf '%s' "$line" | sed -nE 's/.*"error"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*/\1/p' | sed -e 's/\\"/"/g' -e 's/\\\\/\\/g' | cut -c 1-300)" || msg=""
  if [ -z "$msg" ]; then
    msg="$(grep '^ERROR: ' "$SETUP_OUT" 2>/dev/null | tail -n 1 | sed 's/^ERROR: //' | cut -c 1-300 || true)"
  fi
  if [ -z "$msg" ]; then
    msg="$(grep -v '^SETUP_RESULT ' "$SETUP_OUT" 2>/dev/null | last_line)"
  fi
  printf '%s' "${msg:-The setup script failed (exit code $SETUP_RC).}"
}

# restore_old <run setup again: 1 or 0>: put the folder (and the app) back on the old tag.
restore_old() {
  RESTORED_OK=1
  [ "$FROM" != "unknown" ] || return 0
  log "Going back to $FROM"
  if ! git_in_repo checkout -f "$FROM" >>"$LOG" 2>&1; then
    log "Could not check out $FROM again."
    RESTORED_OK=0
    return 0
  fi
  if [ "$1" -eq 1 ]; then
    run_setup
    if ! setup_ok; then
      log "Restoring $FROM failed: $(setup_error)"
      RESTORED_OK=0
    fi
  fi
}

WORK_DIR="$(mktemp -d)"

# ---------------------------------------------------------------- 4. new version
if ! GIT_OUT="$(git_in_repo checkout -f "$NEWEST" 2>&1)"; then
  log "git checkout failed: $GIT_OUT"
  restore_old 0
  fail "Could not switch to $NEWEST: $(printf '%s\n' "$GIT_OUT" | last_line)"
fi
[ -f "$SETUP" ] || { restore_old 0; fail "$NEWEST has no $SETUP_NAME. Ask Claude to repair the install."; }

run_setup
FAIL_MSG=""
if ! setup_ok; then
  FAIL_MSG="$(setup_error)"
  log "Setup failed: $FAIL_MSG"
  case "$FAIL_MSG" in
    *"still being prepared"*)
      FAIL_MSG="This version is still being prepared. Please click Update again in about 30 minutes."
      # Nothing was replaced when setup stopped before its step 3 (the app is replaced inside step 2).
      if grep '^\[3/' "$SETUP_OUT" >/dev/null 2>&1; then restore_old 1; else restore_old 0; fi
      ;;
    *) restore_old 1 ;;
  esac
  if [ "$RESTORED_OK" -eq 0 ]; then FAIL_MSG="$FAIL_MSG (Going back to $FROM also failed.)"; fi
  fail "$FAIL_MSG"
fi

# ---------------------------------------------------------------- 5. health check
if [ ! -x "$MN" ]; then
  FAIL_MSG="The mn tool was not found at $MN after the update."
else
  DOCTOR_OUT="$WORK_DIR/doctor.out"
  DOCTOR_RC=0
  log "Running mn doctor"
  MN_EXPECTED_VERSION="${NEWEST#v}" "$MN" doctor </dev/null >"$DOCTOR_OUT" 2>&1 || DOCTOR_RC=$?
  cat "$DOCTOR_OUT" >>"$LOG" 2>/dev/null || true
  if [ "$DOCTOR_RC" -eq 0 ]; then
    # ---------------------------------------------------------------- 6. success
    if grep -E '"launched"[[:space:]]*:[[:space:]]*false' "$SETUP_OUT" >/dev/null 2>&1; then ensure_app_running; fi
    finish "done" "$TO" ""
    exit 0
  fi
  FAIL_MSG="$(grep -E '^FAIL' "$DOCTOR_OUT" | head -n 1 | sed -E 's/^FAIL[[:space:]]+//' | cut -c 1-300 || true)"
  [ -n "$FAIL_MSG" ] || FAIL_MSG="The health check (mn doctor) failed (exit code $DOCTOR_RC)."
fi

# ---------------------------------------------------------------- 7. failure after setup
log "Health check failed: $FAIL_MSG"
restore_old 1
if [ "$RESTORED_OK" -eq 0 ]; then FAIL_MSG="$FAIL_MSG (Going back to $FROM also failed.)"; fi
fail "$FAIL_MSG"
