#!/usr/bin/env bash
# Meeting Notes setup for Linux. Safe to run again: it updates in place and never touches
# your notes or your My Meetings page address.
#
#   bash scripts/setup/linux.sh --detect-name
#   bash scripts/setup/linux.sh --name "Full Name" [--model small|turbo] [--skip-page] [--skip-launch]
#                               [--release-base <url-or-dir>]
#   bash scripts/setup/linux.sh --update     (an update: the name comes from the existing config)
#
# Last line of output: SETUP_RESULT {json}
set -euo pipefail

DEFAULT_BASE="https://github.com/MustafaAFarag/Meeting-Minutes/releases/latest/download"
TAG_BASE="https://github.com/MustafaAFarag/Meeting-Minutes/releases/download"
CLAUDE_INSTALL_URL="https://claude.ai/install.sh"
WHISPER_URL="https://github.com/ggml-org/whisper.cpp/releases/download/b5130/whisper-bin-ubuntu-x64.tar.gz"
MODEL_URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main"
VAD_URL="https://huggingface.co/ggml-org/whisper-vad/resolve/main"
TOTAL=8

NAME=""
MODEL=""
SKIP_PAGE=0
SKIP_LAUNCH=0
UPDATE=0
RELEASE_BASE=""
RELEASE_TAG=""
RELEASE_LABEL=""
WHISPER_CLI_OVERRIDE=""   # hidden, for tests
WHISPER_MODEL_OVERRIDE="" # hidden, for tests

WARN_JSON=""
STEP=0

json_str() {
  printf '%s' "$1" | tr '\t\r\n' '   ' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# Return a trimmed name only when it looks like a person's name.
usable_name() {
  local candidate login candidate_lower login_lower
  candidate="$(printf '%s' "$1" | tr '\t\r\n' '   ' | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//')" || return 1
  login="${2:-}"
  [ -n "$candidate" ] || return 1
  candidate_lower="$(printf '%s' "$candidate" | tr '[:upper:]' '[:lower:]')" || return 1
  login_lower="$(printf '%s' "$login" | tr '[:upper:]' '[:lower:]')" || return 1
  [ "$candidate_lower" != "$login_lower" ] || return 1
  case "$candidate_lower" in
    admin|administrator|user|owner|root|guest|"microsoft account") return 1 ;;
  esac
  printf '%s' "$candidate" | grep -Eq '[[:alpha:]]' || return 1
  printf '%s' "$candidate"
}

# Email guesses require at least two ASCII-letter parts separated by ., _ or -.
email_name_guess() {
  local email local_part parts part result first rest count
  email="$1"
  case "$email" in *@*) local_part="${email%%@*}" ;; *) return 1 ;; esac
  parts="$(printf '%s' "$local_part" | tr '[:upper:]' '[:lower:]' | awk -F '[-._]' '
    NF < 2 { exit 1 }
    {
      for (i = 1; i <= NF; i++) {
        if ($i !~ /^[a-z]+$/) exit 1
        print $i
      }
    }
  ')" || return 1
  [ -n "$parts" ] || return 1
  result=""
  count=0
  while IFS= read -r part; do
    [ -n "$part" ] || return 1
    first="$(printf '%s' "$part" | cut -c 1 | tr '[:lower:]' '[:upper:]')" || return 1
    rest="$(printf '%s' "$part" | cut -c 2-)" || return 1
    if [ "$count" -gt 0 ]; then result="$result "; fi
    result="${result}${first}${rest}"
    count=$((count + 1))
  done <<EOF
$parts
EOF
  [ "$count" -ge 2 ] || return 1
  printf '%s' "$result"
}

find_claude_for_detection() {
  local c
  if command -v claude >/dev/null 2>&1; then
    command -v claude
    return 0
  fi
  for c in "${HOME:-}/.local/bin/claude" "${HOME:-}/.claude/local/claude" "${HOME:-}/.npm-global/bin/claude" /usr/local/bin/claude; do
    if [ -n "$c" ] && [ -x "$c" ]; then printf '%s' "$c"; return 0; fi
  done
  return 1
}

detect_claude_email_name() {
  local claude timer auth_out email guess
  claude="$(find_claude_for_detection 2>/dev/null)" || return 1
  if command -v timeout >/dev/null 2>&1; then
    timer="$(command -v timeout)"
  elif command -v gtimeout >/dev/null 2>&1; then
    timer="$(command -v gtimeout)"
  else
    return 1
  fi
  auth_out="$("$timer" 3 "$claude" auth status 2>/dev/null)" || return 1
  email="$(printf '%s\n' "$auth_out" | sed -n 's/.*"email"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)" || return 1
  guess="$(email_name_guess "$email" 2>/dev/null)" || return 1
  usable_name "$guess" "${USER:-}"
}

detect_name() {
  local detected source candidate record
  detected=""
  source=""

  if [ -n "${USER:-}" ] && command -v getent >/dev/null 2>&1; then
    if record="$(getent passwd "$USER" 2>/dev/null)"; then
      candidate="$(printf '%s\n' "$record" | awk -F: '{ split($5, gecos, ","); print gecos[1]; exit }' 2>/dev/null || true)"
      detected="$(usable_name "$candidate" "$USER" 2>/dev/null)" || detected=""
    fi
  fi
  if [ -z "$detected" ] && [ -n "${USER:-}" ] && [ -r /etc/passwd ]; then
    record="$(awk -F: -v login="$USER" '$1 == login { print; exit }' /etc/passwd 2>/dev/null || true)"
    candidate="$(printf '%s\n' "$record" | awk -F: '{ split($5, gecos, ","); print gecos[1]; exit }' 2>/dev/null || true)"
    detected="$(usable_name "$candidate" "$USER" 2>/dev/null)" || detected=""
  fi
  if [ -n "$detected" ]; then source="linux-account"; fi

  if [ -z "$detected" ]; then
    detected="$(detect_claude_email_name 2>/dev/null)" || detected=""
    if [ -n "$detected" ]; then source="claude-email-guess"; fi
  fi

  if [ -n "$detected" ]; then
    printf 'DETECTED_NAME {"name":"%s","source":"%s"}\n' "$(json_str "$detected")" "$source"
  else
    printf 'DETECTED_NAME {"name":null,"source":null}\n'
  fi
}

# Detection is deliberately handled before option validation and setup checks.
for arg in "$@"; do
  if [ "$arg" = "--detect-name" ]; then detect_name; exit 0; fi
done

toml_str() {
  printf '"%s"' "$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
}

step() {
  STEP=$((STEP + 1))
  printf '[%d/%d] %s\n' "$STEP" "$TOTAL" "$1"
}

warn() {
  printf 'WARNING: %s\n' "$1"
  if [ -n "$WARN_JSON" ]; then WARN_JSON="$WARN_JSON,"; fi
  WARN_JSON="$WARN_JSON\"$(json_str "$1")\""
}

bool_json() {
  if [ "$1" -eq 1 ]; then printf 'true'; else printf 'false'; fi
}

die() {
  printf 'ERROR: %s\n' "$1" >&2
  printf 'SETUP_RESULT {"ok":false,"update":%s,"error":"%s"}\n' "$(bool_json "$UPDATE")" "$(json_str "$1")"
  exit 1
}

usage() {
  cat <<'USAGE'
Usage: bash scripts/setup/linux.sh --name "Full Name" [options]
  --detect-name             print the detected full name and exit
  --name "Full Name"        the person's full name (asked once; kept from the old config on re-runs)
  --model small|turbo       speech model (default: small)
  --skip-page               do not create the My Meetings page
  --skip-launch             do not start the widget
  --update                  update an existing install (the name is taken from the old config)
  --release-base <url|dir>  where the release files are (default: the GitHub release of the tag this folder is
                            checked out at, or releases/latest/download if it is not at a tag)
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --name) [ "$#" -ge 2 ] || die "--name needs a value."; NAME="$2"; shift 2 ;;
    --name=*) NAME="${1#--name=}"; shift ;;
    --model) [ "$#" -ge 2 ] || die "--model needs small or turbo."; MODEL="$2"; shift 2 ;;
    --model=*) MODEL="${1#--model=}"; shift ;;
    --skip-page) SKIP_PAGE=1; shift ;;
    --skip-launch) SKIP_LAUNCH=1; shift ;;
    --update) UPDATE=1; shift ;;
    --release-base) [ "$#" -ge 2 ] || die "--release-base needs a value."; RELEASE_BASE="$2"; shift 2 ;;
    --release-base=*) RELEASE_BASE="${1#--release-base=}"; shift ;;
    --whisper-cli) [ "$#" -ge 2 ] || die "--whisper-cli needs a value."; WHISPER_CLI_OVERRIDE="$2"; shift 2 ;;
    --whisper-model) [ "$#" -ge 2 ] || die "--whisper-model needs a value."; WHISPER_MODEL_OVERRIDE="$2"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "Unknown option $1. Run with --help to see the options." ;;
  esac
done

# Where the binaries come from. An explicit --release-base always wins. Otherwise: the release of the
# tag this repo folder is checked out at (so going back to an old tag installs the old binaries), or
# the latest release (a dev checkout of main, or no git).
if [ -n "$RELEASE_BASE" ]; then
  RELEASE_BASE="${RELEASE_BASE%/}"
  RELEASE_LABEL="custom"
else
  RELEASE_BASE="$DEFAULT_BASE"
  RELEASE_LABEL="latest"
  REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." 2>/dev/null && pwd)" || REPO_DIR=""
  if [ -n "$REPO_DIR" ] && command -v git >/dev/null 2>&1; then
    TAG="$(git -C "$REPO_DIR" describe --tags --exact-match --match 'v[0-9]*' 2>/dev/null | head -n 1)" || TAG=""
    case "$TAG" in
      v[0-9]*) RELEASE_TAG="$TAG"; RELEASE_LABEL="$TAG"; RELEASE_BASE="$TAG_BASE/$TAG" ;;
    esac
  fi
fi

case "$MODEL" in
  ""|small|turbo) ;;
  *) die "--model must be small or turbo." ;;
esac
[ -n "$MODEL" ] || MODEL="small"

# download <url> <dest>: writes to <dest>.part first, so a broken download never looks finished.
download() {
  local code
  rm -f "$2.part"
  code="$(curl -sSL --retry 3 --retry-delay 3 -o "$2.part" -w '%{http_code}' "$1")" || code="000"
  case "$code" in
    2??) ;;
    404)
      rm -f "$2.part"
      # A tag whose release CI is still building has no files yet.
      if [ -n "$RELEASE_TAG" ] && [ "${1#"$RELEASE_BASE"/}" != "$1" ]; then
        die "This version is still being prepared. Wait 30 minutes, then run this script again."
      fi
      die "Could not download $1. Check the internet connection, then run this script again."
      ;;
    *)
      rm -f "$2.part"
      die "Could not download $1. Check the internet connection, then run this script again."
      ;;
  esac
  mv -f "$2.part" "$2"
}

# fetch_release <file name> <dest>: from the release base (a URL or a local folder).
fetch_release() {
  if [ -d "$RELEASE_BASE" ]; then
    [ -f "$RELEASE_BASE/$1" ] || die "$RELEASE_BASE has no file named $1."
    cp "$RELEASE_BASE/$1" "$2.part"
    mv -f "$2.part" "$2"
  else
    download "$RELEASE_BASE/$1" "$2"
  fi
}

file_size() {
  wc -c <"$1" | tr -d ' '
}

# ---------------------------------------------------------------- 1. checks
step "Checking this computer"
if [ "$RELEASE_LABEL" = "custom" ]; then echo "Using release files from $RELEASE_BASE"; else echo "Using release $RELEASE_LABEL"; fi
[ "$(uname -s)" = "Linux" ] || die "This script is for Linux. Use mac.sh on a Mac or windows.ps1 on Windows."
[ "$(uname -m)" = "x86_64" ] || die "Only 64-bit Intel/AMD Linux (x86_64) is supported."
command -v curl >/dev/null 2>&1 || die "curl is missing. Install curl with the system package manager, then run this script again."
command -v tar >/dev/null 2>&1 || die "tar is missing. Install tar with the system package manager, then run this script again."

CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/meeting-notes"
CONFIG="$CONFIG_DIR/config.toml"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/meeting-notes"
BIN_DIR="$HOME/.local/bin"
MN="$BIN_DIR/mn"
APP="$BIN_DIR/MeetingNotes.AppImage"

if [ "$UPDATE" -eq 1 ] && [ ! -f "$CONFIG" ]; then
  die "There is no earlier install to update (no config file at $CONFIG). Do a normal install instead: follow SETUP.md from step 0 and run this script with --name \"Full Name\"."
fi
mkdir -p "$CONFIG_DIR" "$DATA_DIR" "$BIN_DIR"

# The standalone Claude Code that 'mn' calls in the background. It is installed here with the
# official installer when missing (a file, never piped into a shell).
CLAUDE="$(find_claude_for_detection 2>/dev/null || true)"
if [ -z "$CLAUDE" ]; then
  echo "Installing Claude Code (about 1 minute)..."
  download "$CLAUDE_INSTALL_URL" "$DATA_DIR/claude-install.sh"
  bash "$DATA_DIR/claude-install.sh" >"$DATA_DIR/claude-install.log" 2>&1 ||
    die "Could not install Claude Code ($(tail -n 1 "$DATA_DIR/claude-install.log" | tr -d '\r')). Check the internet connection, then run this script again."
  rm -f "$DATA_DIR/claude-install.sh" "$DATA_DIR/claude-install.log"
  CLAUDE="$(find_claude_for_detection 2>/dev/null || true)"
  [ -n "$CLAUDE" ] || die "Claude Code was installed but was not found at $HOME/.local/bin/claude. Run this script again."
fi

AUTH_OUT=""
AUTH_OK=1
AUTH_OUT="$("$CLAUDE" auth status 2>&1)" || AUTH_OK=0
if printf '%s' "$AUTH_OUT" | grep -Eq '"loggedIn"[[:space:]]*:[[:space:]]*false'; then AUTH_OK=0; fi
if [ "$AUTH_OK" -eq 0 ]; then
  if [ "$SKIP_PAGE" -eq 1 ]; then
    warn "Claude Code is not signed in. Do the sign-in step of SETUP.md (step 2) before the first recording."
  else
    die "Claude Code is not signed in. Do the sign-in step of SETUP.md (step 2): run '$CLAUDE auth login' (the user clicks Authorize in the browser), or have the user type 'claude' in a terminal and sign in. Then run this script again."
  fi
fi
CODEX="$(command -v codex 2>/dev/null || true)"

# Keep the name and the notes folder of an earlier install when they are not given again.
OLD_NOTES_LINE=""
OLD_NAME_LINE=""
if [ -f "$CONFIG" ]; then
  OLD_NOTES_LINE="$(grep -m1 -E '^notes_dir[[:space:]]*=' "$CONFIG" || true)"
  OLD_NAME_LINE="$(grep -m1 -E '^my_name[[:space:]]*=' "$CONFIG" || true)"
fi
if [ -z "$NAME" ] && [ -z "$OLD_NAME_LINE" ]; then
  if [ "$UPDATE" -eq 1 ]; then
    die "The existing config at $CONFIG has no name, so this cannot be an update. Do a normal install instead: follow SETUP.md from step 0 and run this script with --name \"Full Name\"."
  fi
  die "The person's full name is missing. Ask the user for their full name and run this script again with --name \"Full Name\"."
fi

MISSING_AUDIO=""
for tool in pactl parec; do
  command -v "$tool" >/dev/null 2>&1 || MISSING_AUDIO="$MISSING_AUDIO $tool"
done
if [ -n "$MISSING_AUDIO" ]; then
  warn "Missing audio tools:$MISSING_AUDIO. Install pipewire-pulse (Arch/Fedora) or pulseaudio-utils (Ubuntu/Debian), or recording will not work."
fi
# Not `grep -q`: it exits at the first match, ldconfig then dies of SIGPIPE, and pipefail
# turns that into "not found" even when libfuse2 is installed.
if ! { ldconfig -p 2>/dev/null | grep 'libfuse\.so\.2' >/dev/null; }; then
  warn "libfuse2 was not found; the AppImage may not start. Install fuse2 (Arch) or libfuse2 (Ubuntu/Debian)."
fi

# ---------------------------------------------------------------- 2. app
step "Installing the Meeting Notes app (about 1 minute)"
fetch_release "MeetingNotes-linux.AppImage" "$APP"
chmod +x "$APP"

# An application-menu entry, so the widget can be opened again. No Icon line: the AppImage is not unpacked here.
DESKTOP_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
case "$APP" in
  *[\\\"\$\`]*)
    warn "The app path has special characters, so the application-menu entry was not written. Start $APP from a terminal."
    ;;
  *)
    if mkdir -p "$DESKTOP_DIR" && printf '[Desktop Entry]\nType=Application\nName=Meeting Notes\nComment=Record calls and get meeting notes\nExec="%s"\nTerminal=false\nCategories=Office;AudioVideo;\n' "$APP" >"$DESKTOP_DIR/meeting-notes.desktop"; then
      chmod +x "$DESKTOP_DIR/meeting-notes.desktop" 2>/dev/null || true
    else
      warn "Could not write the application-menu entry in $DESKTOP_DIR."
    fi
    ;;
esac

# ---------------------------------------------------------------- 3. mn
step "Installing the command-line tool (mn)"
fetch_release "mn-linux" "$MN"
chmod +x "$MN"
"$MN" --version >/dev/null 2>&1 || die "The downloaded mn tool does not run on this computer."

# ---------------------------------------------------------------- 4. whisper
step "Installing whisper (the speech-to-text engine)"
WHISPER=""
FOUND=""
if [ -d "$DATA_DIR/whisper" ]; then
  FOUND="$(find "$DATA_DIR/whisper" -name whisper-cli -type f -perm -u+x 2>/dev/null | head -n 1 || true)"
fi
if [ -n "$WHISPER_CLI_OVERRIDE" ]; then
  [ -x "$WHISPER_CLI_OVERRIDE" ] || die "--whisper-cli $WHISPER_CLI_OVERRIDE is not an executable file."
  WHISPER="$WHISPER_CLI_OVERRIDE"
elif [ -n "$FOUND" ]; then
  WHISPER="$FOUND"
  echo "Already installed: $WHISPER"
elif command -v whisper-cli >/dev/null 2>&1; then
  WHISPER="$(command -v whisper-cli)"
  echo "Using the whisper-cli already on this computer: $WHISPER"
else
  rm -rf "$DATA_DIR/whisper" "$DATA_DIR/whisper.tar.gz"
  mkdir -p "$DATA_DIR/whisper"
  echo "Downloading whisper (about 15 MB)..."
  download "$WHISPER_URL" "$DATA_DIR/whisper.tar.gz"
  tar -xzf "$DATA_DIR/whisper.tar.gz" -C "$DATA_DIR/whisper" || die "Could not unpack the whisper download. Run this script again."
  rm -f "$DATA_DIR/whisper.tar.gz"
  WHISPER="$(find "$DATA_DIR/whisper" -name whisper-cli -type f | head -n 1)"
  [ -n "$WHISPER" ] || die "The whisper download did not contain whisper-cli. Run this script again."
  chmod +x "$WHISPER"
fi
"$WHISPER" --help >/dev/null 2>&1 || warn "whisper-cli at $WHISPER did not start. A system library may be missing (try installing libgomp)."

# ---------------------------------------------------------------- 5. model
step "Getting the speech model"
if [ -n "$WHISPER_MODEL_OVERRIDE" ]; then
  [ -f "$WHISPER_MODEL_OVERRIDE" ] || die "--whisper-model $WHISPER_MODEL_OVERRIDE does not exist."
  WMODEL="$WHISPER_MODEL_OVERRIDE"
else
  if [ "$MODEL" = "turbo" ]; then
    MODEL_FILE="ggml-large-v3-turbo-q5_0.bin"; MODEL_MIN=520000000; MODEL_MB=574
  else
    MODEL_FILE="ggml-small.bin"; MODEL_MIN=440000000; MODEL_MB=488
  fi
  mkdir -p "$DATA_DIR/models"
  WMODEL="$DATA_DIR/models/$MODEL_FILE"
  if [ -f "$WMODEL" ] && [ "$(file_size "$WMODEL")" -ge "$MODEL_MIN" ]; then
    echo "Already downloaded: $WMODEL"
  else
    echo "Downloading $MODEL_FILE (about $MODEL_MB MB, a few minutes)..."
    download "$MODEL_URL/$MODEL_FILE" "$WMODEL"
    [ "$(file_size "$WMODEL")" -ge "$MODEL_MIN" ] || die "The speech model download is incomplete. Run this script again."
  fi
fi

# Silero VAD (about 1 MB): whisper then skips silence, so it cannot invent lines there, and
# "auto" picks the language from the first speech instead of from a silent start.
VAD_FILE="ggml-silero-v5.1.2.bin"; VAD_SIZE=885098
mkdir -p "$DATA_DIR/models"
VMODEL="$DATA_DIR/models/$VAD_FILE"
if [ ! -f "$VMODEL" ] || [ "$(file_size "$VMODEL")" != "$VAD_SIZE" ]; then
  download "$VAD_URL/$VAD_FILE" "$VMODEL"
  [ "$(file_size "$VMODEL")" = "$VAD_SIZE" ] || die "The VAD model download is incomplete. Run this script again."
fi

# ---------------------------------------------------------------- 6. config
step "Writing the config"
if [ -n "$OLD_NOTES_LINE" ]; then
  NOTES_LINE="$OLD_NOTES_LINE"
  NOTES_DIR="$(printf '%s' "$OLD_NOTES_LINE" | sed -E "s/^notes_dir[[:space:]]*=[[:space:]]*[\"']//; s/[\"'][[:space:]]*\$//")"
else
  NOTES_DIR="$HOME/MeetingNotes"
  mkdir -p "$NOTES_DIR"
  NOTES_LINE="notes_dir = $(toml_str "$NOTES_DIR")"
fi
if [ -n "$NAME" ]; then NAME_LINE="my_name = $(toml_str "$NAME")"; else NAME_LINE="$OLD_NAME_LINE"; fi

if [ -f "$CONFIG" ]; then cp -f "$CONFIG" "$CONFIG.bak"; fi
{
  echo "# Written by the Meeting Notes setup script. Re-running setup rewrites this file (the old one is kept as config.toml.bak)."
  echo "$NOTES_LINE"
  echo "whisper_cli = $(toml_str "$WHISPER")"
  echo "whisper_model = $(toml_str "$WMODEL")"
  echo "vad_model = $(toml_str "$VMODEL")"
  echo "claude_cli = $(toml_str "$CLAUDE")"
  if [ -n "$CODEX" ]; then echo "codex_cli = $(toml_str "$CODEX")"; fi
  if [ -n "${CLAUDE_CONFIG_DIR:-}" ]; then echo "claude_config_dir = $(toml_str "$CLAUDE_CONFIG_DIR")"; fi
  echo "$NAME_LINE"
  echo 'language = "auto"'
} >"$CONFIG.new"
mv -f "$CONFIG.new" "$CONFIG"
"$MN" config >/dev/null 2>&1 || die "The config file at $CONFIG is not valid. Show this path to the developer."

# ---------------------------------------------------------------- 7. page
step "Creating the My Meetings page (1 to 2 minutes)"
URL_FILE="$CONFIG_DIR/artifact_url.txt"
PAGE_URL=""
if [ "$SKIP_PAGE" -eq 1 ]; then
  echo "Skipped (--skip-page)."
else
  PAGE_LOG="$DATA_DIR/init-page.log"
  PAGE_OK=1
  "$MN" init-page >"$PAGE_LOG" 2>&1 || PAGE_OK=0
  PAGE_URL="$(grep '"state":"published"' "$PAGE_LOG" | tail -n 1 | sed -E 's/.*"url":"([^"]*)".*/\1/' || true)"
  if [ "$PAGE_OK" -eq 0 ] || [ -z "$PAGE_URL" ]; then
    PAGE_ERR="$(grep -E '"state":"error"|^error:' "$PAGE_LOG" | tail -n 1 || true)"
    warn "mn init-page failed: ${PAGE_ERR:-no message, see $PAGE_LOG}. Run '$MN init-page' again and show this error to the developer."
    PAGE_URL=""
  fi
fi
if [ -z "$PAGE_URL" ] && [ -f "$URL_FILE" ]; then
  PAGE_URL="$(head -n 1 "$URL_FILE" | tr -d '\r')"
fi

# ---------------------------------------------------------------- 8. launch
step "Starting the widget"
LAUNCHED=false
if [ "$SKIP_LAUNCH" -eq 1 ]; then
  echo "Skipped (--skip-launch)."
elif [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
  warn "No graphical session found, so the widget was not started. Start $APP from the desktop."
else
  if pgrep -f "$APP" >/dev/null 2>&1; then
    pkill -f "$APP" >/dev/null 2>&1 || true
    sleep 2
  fi
  setsid nohup "$APP" >"$DATA_DIR/app.log" 2>&1 </dev/null &
  APP_PID=$!
  sleep 4
  if kill -0 "$APP_PID" 2>/dev/null; then
    LAUNCHED=true
    echo "The widget is running. It turns on start-at-login by itself on the first run."
  else
    warn "The widget stopped right after starting. See $DATA_DIR/app.log (often a missing libfuse2)."
  fi
fi

if [ -n "$PAGE_URL" ]; then PAGE_JSON="\"$(json_str "$PAGE_URL")\""; else PAGE_JSON="null"; fi
printf 'SETUP_RESULT {"ok":true,"os":"linux","config":"%s","artifact_url_file":"%s","mn":"%s","app":"%s","whisper_cli":"%s","whisper_model":"%s","notes_dir":"%s","claude_cli":"%s","page_url":%s,"launched":%s,"update":%s,"release":"%s","warnings":[%s]}\n' \
  "$(json_str "$CONFIG")" "$(json_str "$URL_FILE")" "$(json_str "$MN")" "$(json_str "$APP")" \
  "$(json_str "$WHISPER")" "$(json_str "$WMODEL")" "$(json_str "$NOTES_DIR")" "$(json_str "$CLAUDE")" \
  "$PAGE_JSON" "$LAUNCHED" "$(bool_json "$UPDATE")" "$(json_str "$RELEASE_LABEL")" "$WARN_JSON"
