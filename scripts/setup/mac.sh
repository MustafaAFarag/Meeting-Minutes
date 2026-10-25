#!/bin/bash
# Meeting Notes setup for macOS. Safe to run again: it updates in place and never touches
# your notes or your My Meetings page address. Works with the bash 3.2 that ships with macOS.
#
#   bash scripts/setup/mac.sh --detect-name
#   bash scripts/setup/mac.sh --name "Full Name" [--model small|turbo] [--skip-page] [--skip-launch]
#                             [--release-base <url-or-dir>]
#   bash scripts/setup/mac.sh --update     (an update: the name comes from the existing config)
#
# Last line of output: SETUP_RESULT {json}
set -euo pipefail

DEFAULT_BASE="https://github.com/MustafaAFarag/Meeting-Minutes/releases/latest/download"
TAG_BASE="https://github.com/MustafaAFarag/Meeting-Minutes/releases/download"
CLAUDE_INSTALL_URL="https://claude.ai/install.sh"
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
WORK_DIR=""
MOUNT_DIR=""

cleanup() {
  if [ -n "$MOUNT_DIR" ]; then
    hdiutil detach "$MOUNT_DIR" -quiet >/dev/null 2>&1 || hdiutil detach "$MOUNT_DIR" -force -quiet >/dev/null 2>&1 || true
    MOUNT_DIR=""
  fi
  if [ -n "$WORK_DIR" ]; then rm -rf "$WORK_DIR"; fi
}
trap cleanup EXIT

json_str() {
  printf '%s' "$1" | tr '\t\r\n' '   ' | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

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
  for c in "${HOME:-}/.local/bin/claude" "${HOME:-}/.claude/local/claude" /opt/homebrew/bin/claude /usr/local/bin/claude "${HOME:-}/.npm-global/bin/claude"; do
    if [ -n "$c" ] && [ -x "$c" ]; then printf '%s' "$c"; return 0; fi
  done
  return 1
}

detect_claude_email_name() {
  local claude timer auth_out email guess
  claude="$(find_claude_for_detection 2>/dev/null)" || return 1
  if command -v timeout >/dev/null 2>&1; then
    timer="$(command -v timeout)"
    auth_out="$("$timer" 3 "$claude" auth status 2>/dev/null)" || return 1
  elif command -v gtimeout >/dev/null 2>&1; then
    timer="$(command -v gtimeout)"
    auth_out="$("$timer" 3 "$claude" auth status 2>/dev/null)" || return 1
  elif command -v perl >/dev/null 2>&1; then
    # macOS has no timeout command; perl ships with it and can do the same.
    auth_out="$(perl -e 'alarm shift; exec @ARGV' 3 "$claude" auth status 2>/dev/null)" || return 1
  else
    return 1
  fi
  email="$(printf '%s\n' "$auth_out" | sed -n 's/.*"email"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)" || return 1
  guess="$(email_name_guess "$email" 2>/dev/null)" || return 1
  usable_name "$guess" "${USER:-}"
}

detect_name() {
  local detected source candidate
  detected=""
  source=""

  if candidate="$(id -F 2>/dev/null)"; then
    detected="$(usable_name "$candidate" "${USER:-}" 2>/dev/null)" || detected=""
  fi
  if [ -z "$detected" ] && candidate="$(dscl . -read "/Users/${USER:-}" RealName 2>/dev/null | awk '
    NR == 1 {
      sub(/^RealName:[[:space:]]*/, "")
      if (length($0) > 0) value = $0
    }
    NR > 1 && value == "" {
      sub(/^[[:space:]]*/, "")
      if (length($0) > 0) value = $0
    }
    END { if (value != "") print value }
  ')"; then
    detected="$(usable_name "$candidate" "${USER:-}" 2>/dev/null)" || detected=""
  fi
  if [ -n "$detected" ]; then source="macos-account"; fi

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
Usage: bash scripts/setup/mac.sh --name "Full Name" [options]
  --detect-name             print the detected full name and exit
  --name "Full Name"        the person's full name (asked once; kept from the old config on re-runs)
  --model small|turbo       speech model (default: turbo on Apple Silicon, small on Intel)
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
# the latest release (a dev checkout of main, or no git). On a Mac without the developer tools, git is
# only a stub that opens Apple's install window, so it is not called then.
if [ -n "$RELEASE_BASE" ]; then
  RELEASE_BASE="${RELEASE_BASE%/}"
  RELEASE_LABEL="custom"
  # The files are run right after the download: only this project's releases or a local folder.
  case "$RELEASE_BASE" in
    "${TAG_BASE%/download}"/*) ;;
    *) [ -d "$RELEASE_BASE" ] || [ "${MN_ALLOW_CUSTOM_RELEASE_BASE:-}" = "1" ] ||
      die "--release-base must be a local folder or a release of this project on GitHub." ;;
  esac
else
  RELEASE_BASE="$DEFAULT_BASE"
  RELEASE_LABEL="latest"
  REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." 2>/dev/null && pwd)" || REPO_DIR=""
  if [ -n "$REPO_DIR" ] && xcode-select -p >/dev/null 2>&1 && command -v git >/dev/null 2>&1; then
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

# Prints the whisper-cli that is already installed, or nothing.
find_whisper() {
  local c
  for c in /opt/homebrew/bin/whisper-cli /usr/local/bin/whisper-cli "$(command -v whisper-cli 2>/dev/null || true)"; do
    if [ -n "$c" ] && [ -x "$c" ]; then printf '%s' "$c"; return 0; fi
  done
  return 0
}

# Prints the brew executable, or nothing.
find_brew() {
  local c
  for c in "$(command -v brew 2>/dev/null || true)" /opt/homebrew/bin/brew /usr/local/bin/brew; do
    if [ -n "$c" ] && [ -x "$c" ]; then printf '%s' "$c"; return 0; fi
  done
  return 0
}

# Puts the app in the Dock unless a tile for it is already there. The app has no Dock icon while it
# runs, so the tile is how the user opens it again. A failure here is only a warning.
add_to_dock() {
  local app="$1" encoded tile
  encoded="$(perl -e 'print join("", map { /[A-Za-z0-9_.\/~-]/ ? $_ : sprintf("%%%02X", ord($_)) } split(//, $ARGV[0]))' "$app")" || return 1
  [ -n "$encoded" ] || return 1
  if defaults read com.apple.dock persistent-apps 2>/dev/null | grep -F "file://$encoded" >/dev/null 2>&1; then
    return 0
  fi
  tile="<dict><key>tile-data</key><dict><key>file-data</key><dict><key>_CFURLString</key><string>file://$encoded/</string><key>_CFURLStringType</key><integer>15</integer></dict></dict></dict>"
  defaults write com.apple.dock persistent-apps -array-add "$tile" || return 1
  killall Dock >/dev/null 2>&1 || true
}

# ---------------------------------------------------------------- 1. checks
step "Checking this computer"
if [ "$RELEASE_LABEL" = "custom" ]; then echo "Using release files from $RELEASE_BASE"; else echo "Using release $RELEASE_LABEL"; fi
[ "$(uname -s)" = "Darwin" ] || die "This script is for a Mac. Use linux.sh on Linux or windows.ps1 on Windows."
command -v curl >/dev/null 2>&1 || die "curl is missing. This Mac looks broken; ask the developer."

MACOS_VERSION="$(sw_vers -productVersion 2>/dev/null || echo 0.0)"
MACOS_MAJOR="${MACOS_VERSION%%.*}"
MACOS_MINOR="$(printf '%s' "$MACOS_VERSION" | cut -d. -f2)"
MACOS_MINOR="${MACOS_MINOR:-0}"
if [ "$MACOS_MAJOR" -lt 14 ] || { [ "$MACOS_MAJOR" -eq 14 ] && [ "$MACOS_MINOR" -lt 2 ]; }; then
  warn "macOS $MACOS_VERSION is older than 14.2: the other side of a call cannot be recorded (only the microphone). Update macOS to fix this."
fi

APPLE_SILICON=0
if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || echo 0)" = "1" ]; then APPLE_SILICON=1; fi
if [ -z "$MODEL" ]; then
  if [ "$APPLE_SILICON" -eq 1 ]; then MODEL="turbo"; else MODEL="small"; fi
fi

CONFIG_DIR="$HOME/Library/Application Support/meeting-notes"
CONFIG="$CONFIG_DIR/config.toml"
WHISPER_BIN_DIR="$CONFIG_DIR/bin"
WHISPER_BIN="$WHISPER_BIN_DIR/whisper-cli"
BIN_DIR="$HOME/.local/bin"
MN="$BIN_DIR/mn"

if [ "$UPDATE" -eq 1 ] && [ ! -f "$CONFIG" ]; then
  die "There is no earlier install to update (no config file at $CONFIG). Do a normal install instead: follow SETUP.md from step 0 and run this script with --name \"Full Name\"."
fi

WORK_DIR="$(mktemp -d)"

# The standalone Claude Code that 'mn' calls in the background. Claude Desktop's own sign-in is not
# shared with it, so it is installed here with the official installer (a file, never piped into a shell).
CLAUDE="$(find_claude_for_detection 2>/dev/null || true)"
if [ -z "$CLAUDE" ]; then
  echo "Installing Claude Code (about 1 minute)..."
  download "$CLAUDE_INSTALL_URL" "$WORK_DIR/claude-install.sh"
  bash "$WORK_DIR/claude-install.sh" >"$WORK_DIR/claude-install.log" 2>&1 ||
    die "Could not install Claude Code ($(tail -n 1 "$WORK_DIR/claude-install.log" | tr -d '\r')). Check the internet connection, then run this script again."
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
    die "Claude Code is not signed in. Do the sign-in step of SETUP.md (step 2): run '$CLAUDE auth login' (the user clicks Authorize in the browser), or have the user type 'claude' in Terminal and sign in. Then run this script again."
  fi
fi
CODEX="$(command -v codex 2>/dev/null || true)"

mkdir -p "$CONFIG_DIR" "$WHISPER_BIN_DIR" "$BIN_DIR"

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

# ---------------------------------------------------------------- 2. app
step "Installing the Meeting Notes app (about 1 minute)"
fetch_release "MeetingNotes-macos.dmg" "$WORK_DIR/MeetingNotes-macos.dmg"
MOUNT_DIR="$WORK_DIR/mount"
mkdir -p "$MOUNT_DIR"
hdiutil attach -nobrowse -readonly -mountpoint "$MOUNT_DIR" "$WORK_DIR/MeetingNotes-macos.dmg" >/dev/null ||
  die "Could not open the downloaded disk image. Run this script again."
APP_SRC="$(find "$MOUNT_DIR" -maxdepth 1 -name '*.app' | head -n 1)"
[ -n "$APP_SRC" ] || die "The disk image has no app inside. Run this script again."
APP_BASE="$(basename "$APP_SRC")"

if [ -w /Applications ]; then
  APP_DIR="/Applications"
else
  APP_DIR="$HOME/Applications"
  mkdir -p "$APP_DIR"
fi
APP="$APP_DIR/$APP_BASE"

# Quit a running copy so the files can be replaced.
osascript -e "tell application \"${APP_BASE%.app}\" to quit" >/dev/null 2>&1 || true
sleep 1
pkill -f "$APP_BASE/Contents/MacOS" >/dev/null 2>&1 || true
for other in "/Applications/$APP_BASE" "$HOME/Applications/$APP_BASE"; do
  if [ "$other" != "$APP" ] && [ -d "$other" ]; then rm -rf "$other" 2>/dev/null || true; fi
done
rm -rf "$APP"
ditto "$APP_SRC" "$APP"
hdiutil detach "$MOUNT_DIR" -quiet >/dev/null 2>&1 || hdiutil detach "$MOUNT_DIR" -force -quiet >/dev/null 2>&1 || true
MOUNT_DIR=""
# The app is unsigned: remove the quarantine flag so Gatekeeper does not block it.
xattr -dr com.apple.quarantine "$APP" >/dev/null 2>&1 || true
if ! add_to_dock "$APP"; then
  warn "Could not add Meeting Notes to the Dock. The user can open it from Spotlight (Command+Space, type Meeting Notes)."
fi

# ---------------------------------------------------------------- 3. mn
step "Installing the command-line tool (mn)"
fetch_release "mn-macos" "$MN"
chmod +x "$MN"
xattr -d com.apple.quarantine "$MN" >/dev/null 2>&1 || true
codesign --force --sign - "$MN" >/dev/null 2>&1 || true
"$MN" --version >/dev/null 2>&1 || die "The downloaded mn tool does not run on this Mac."

# ---------------------------------------------------------------- 4. whisper
step "Installing whisper (the speech-to-text engine)"
WHISPER=""
if [ -n "$WHISPER_CLI_OVERRIDE" ]; then
  [ -x "$WHISPER_CLI_OVERRIDE" ] || die "--whisper-cli $WHISPER_CLI_OVERRIDE is not an executable file."
  WHISPER="$WHISPER_CLI_OVERRIDE"
else
  if [ -x "$WHISPER_BIN" ]; then
    WHISPER="$WHISPER_BIN"
  else
    WHISPER="$(find_whisper)"
  fi
  if [ -n "$WHISPER" ]; then
    echo "Already installed: $WHISPER"
  else
    rm -f "$WHISPER_BIN" "$WHISPER_BIN.part"
    if (
      fetch_release "whisper-cli-macos" "$WHISPER_BIN" &&
        chmod +x "$WHISPER_BIN" &&
        { xattr -d com.apple.quarantine "$WHISPER_BIN" >/dev/null 2>&1 || true; } &&
        { codesign --force --sign - "$WHISPER_BIN" >/dev/null 2>&1 || true; } &&
        "$WHISPER_BIN" --help >/dev/null 2>&1
    ) >/dev/null 2>&1; then
      WHISPER="$WHISPER_BIN"
      echo "Downloaded: $WHISPER"
    else
      rm -f "$WHISPER_BIN" "$WHISPER_BIN.part"
      BREW="$(find_brew)"
      [ -n "$BREW" ] || die "Could not install whisper. Ask the user to install Homebrew: open Terminal, paste the command from https://brew.sh, press Enter and type their Mac password when asked. Then run this script again."
      echo "Running: brew install whisper-cpp (a few minutes)..."
      HOMEBREW_NO_ENV_HINTS=1 "$BREW" install whisper-cpp ||
        die "brew could not install whisper-cpp. Run '$BREW install whisper-cpp' in Terminal, fix what it says, then run this script again."
      WHISPER="$("$BREW" --prefix)/bin/whisper-cli"
      [ -x "$WHISPER" ] || die "brew installed whisper-cpp but $WHISPER is missing. Show this message to the developer."
      warn "The whisper download failed, so Homebrew was used as a fallback and may have asked for your Mac password."
    fi
  fi
fi
"$WHISPER" --help >/dev/null 2>&1 || warn "whisper-cli at $WHISPER did not start. Run 'brew reinstall whisper-cpp' or ask the developer."

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
  mkdir -p "$CONFIG_DIR/models"
  WMODEL="$CONFIG_DIR/models/$MODEL_FILE"
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
mkdir -p "$CONFIG_DIR/models"
VMODEL="$CONFIG_DIR/models/$VAD_FILE"
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
  PAGE_LOG="$WORK_DIR/init-page.log"
  PAGE_OK=1
  "$MN" init-page >"$PAGE_LOG" 2>&1 || PAGE_OK=0
  PAGE_URL="$(grep '"state":"published"' "$PAGE_LOG" | tail -n 1 | sed -E 's/.*"url":"([^"]*)".*/\1/' || true)"
  if [ "$PAGE_OK" -eq 0 ] || [ -z "$PAGE_URL" ]; then
    PAGE_ERR="$(grep -E '"state":"error"|^error:' "$PAGE_LOG" | tail -n 1 || true)"
    warn "mn init-page failed: ${PAGE_ERR:-no message}. Run '$MN init-page' again and show this error to the developer."
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
else
  if open "$APP" >/dev/null 2>&1; then
    sleep 4
    if pgrep -f "$APP_BASE/Contents/MacOS" >/dev/null 2>&1; then
      LAUNCHED=true
      echo "The widget is running. It turns on start-at-login by itself on the first run."
    else
      warn "The widget did not stay open. Open it from $APP_DIR (right-click, Open) and read any macOS message."
    fi
  else
    warn "macOS would not open the app. Right-click it in $APP_DIR, choose Open, then Open again."
  fi
fi

if [ -n "$PAGE_URL" ]; then PAGE_JSON="\"$(json_str "$PAGE_URL")\""; else PAGE_JSON="null"; fi
printf 'SETUP_RESULT {"ok":true,"os":"macos","config":"%s","artifact_url_file":"%s","mn":"%s","app":"%s","whisper_cli":"%s","whisper_model":"%s","notes_dir":"%s","claude_cli":"%s","page_url":%s,"launched":%s,"update":%s,"release":"%s","warnings":[%s]}\n' \
  "$(json_str "$CONFIG")" "$(json_str "$URL_FILE")" "$(json_str "$MN")" "$(json_str "$APP")" \
  "$(json_str "$WHISPER")" "$(json_str "$WMODEL")" "$(json_str "$NOTES_DIR")" "$(json_str "$CLAUDE")" \
  "$PAGE_JSON" "$LAUNCHED" "$(bool_json "$UPDATE")" "$(json_str "$RELEASE_LABEL")" "$WARN_JSON"
