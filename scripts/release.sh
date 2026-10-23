#!/usr/bin/env bash
# Developer tool: cut a release. It bumps the version, checks the gates, commits and tags. It never pushes.
#
#   bash scripts/release.sh 0.1.6             do it
#   bash scripts/release.sh 0.1.6 --dry-run   print what it would change and run nothing
#
# Needs: you are on main with a clean tree, the version is X.Y.Z and higher than the current one,
# and CHANGELOG.md already has a "## v0.1.6" heading (it may say "unreleased"; that becomes today's date).
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=""
DRY=0

usage() {
  cat <<'USAGE'
Usage: bash scripts/release.sh X.Y.Z [--dry-run]
  X.Y.Z      the new version (higher than the current one)
  --dry-run  print what would change and run nothing
It changes core/Cargo.toml, app/src-tauri/Cargo.toml, app/src-tauri/tauri.conf.json, both Cargo.lock files
and CHANGELOG.md, runs the gates, commits "release: vX.Y.Z" and makes the annotated tag vX.Y.Z. It does not push.
USAGE
}

die() {
  printf 'ERROR: %s\n' "$1" >&2
  exit 1
}

# problem <message>: stop on a real run; on a dry run say so and go on, so the whole plan is shown.
problem() {
  if [ "$DRY" -eq 1 ]; then
    printf 'NOTE (a real run would stop here): %s\n' "$1"
  else
    die "$1"
  fi
}

for arg in "$@"; do
  case "$arg" in
    --dry-run) DRY=1 ;;
    -h|--help) usage; exit 0 ;;
    -*) die "Unknown option $arg. Run with --help." ;;
    *) [ -z "$VERSION" ] || die "Give one version only."; VERSION="$arg" ;;
  esac
done
[ -n "$VERSION" ] || { usage; exit 1; }
printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || die "The version must look like 0.1.6 (three numbers, no v)."
TAG="v$VERSION"

CORE_TOML="core/Cargo.toml"
APP_TOML="app/src-tauri/Cargo.toml"
TAURI_CONF="app/src-tauri/tauri.conf.json"
for f in "$CORE_TOML" "$APP_TOML" "$TAURI_CONF" CHANGELOG.md; do
  [ -f "$f" ] || die "$f is missing. Run this from the repo."
done

# The version line inside [package] only (not a dependency's version).
package_version() {
  awk '
    /^\[/ { in_pkg = ($0 == "[package]") }
    in_pkg && /^version[[:space:]]*=/ { gsub(/^[^"]*"|".*$/, ""); print; exit }
  ' "$1"
}

set_package_version() {
  local file="$1" tmp
  tmp="$file.release.tmp"
  awk -v v="$VERSION" '
    /^\[/ { in_pkg = ($0 == "[package]") }
    in_pkg && !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
    { print }
  ' "$file" >"$tmp"
  mv -f "$tmp" "$file"
}

tauri_version() {
  sed -n -E 's/^  "version": "([^"]*)".*/\1/p' "$TAURI_CONF" | head -n 1
}

CURRENT="$(package_version "$CORE_TOML")"
APP_CURRENT="$(package_version "$APP_TOML")"
TAURI_CURRENT="$(tauri_version)"
[ -n "$CURRENT" ] || die "Could not read the version from $CORE_TOML."

# ---------------------------------------------------------------- checks
BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[ "$BRANCH" = "main" ] || problem "You are on '$BRANCH'. Releases are made from main."

if [ -n "$(git status --porcelain --untracked-files=no)" ]; then
  problem "The working tree has uncommitted changes. Commit or stash them first."
fi

if [ "$APP_CURRENT" != "$CURRENT" ] || [ "$TAURI_CURRENT" != "$CURRENT" ]; then
  problem "The versions are out of step: core $CURRENT, app $APP_CURRENT, tauri.conf.json $TAURI_CURRENT. Make them equal first."
fi

if [ "$VERSION" = "$CURRENT" ] || [ "$(printf '%s\n%s\n' "$CURRENT" "$VERSION" | sort -V | tail -n 1)" != "$VERSION" ]; then
  problem "$VERSION is not higher than the current version $CURRENT."
fi

if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null; then
  problem "The tag $TAG already exists."
fi

HEADING="$(grep -E "^## $TAG( |\$)" CHANGELOG.md | head -n 1 || true)"
if [ -z "$HEADING" ]; then
  problem "CHANGELOG.md has no '## $TAG' heading. Write the entry first (\"What's new for you\" and \"Technical\")."
fi

TODAY="$(date +%Y-%m-%d)"

# ---------------------------------------------------------------- the plan
echo "Release $TAG (current version: $CURRENT)"
echo "  $CORE_TOML                    version $CURRENT -> $VERSION"
echo "  $APP_TOML        version $APP_CURRENT -> $VERSION"
echo "  $TAURI_CONF  version $TAURI_CURRENT -> $VERSION"
echo "  core/Cargo.lock, app/src-tauri/Cargo.lock   refreshed offline (cargo update --workspace --offline), other crates unchanged"
if printf '%s' "$HEADING" | grep -q 'unreleased'; then
  echo "  CHANGELOG.md                         '$HEADING' -> '## $TAG - $TODAY'"
else
  echo "  CHANGELOG.md                         heading '$HEADING' already has a date; left as it is"
fi
echo "  gates                                cargo test, cargo clippy -- -D warnings, cargo fmt --check in core/ and app/src-tauri/; bash -n on the scripts"
echo "  git                                  commit \"release: $TAG\", annotated tag $TAG (nothing is pushed)"

if [ "$DRY" -eq 1 ]; then
  echo
  echo "Dry run: nothing was changed."
  echo "A real run ends by printing: git push origin main $TAG"
  exit 0
fi

# ---------------------------------------------------------------- edits
set_package_version "$CORE_TOML"
set_package_version "$APP_TOML"
sed -i.release.bak -E "s/^(  \"version\": \")[^\"]*(\")/\1$VERSION\2/" "$TAURI_CONF"
rm -f "$TAURI_CONF.release.bak"
[ "$(package_version "$CORE_TOML")" = "$VERSION" ] || die "Could not set the version in $CORE_TOML."
[ "$(package_version "$APP_TOML")" = "$VERSION" ] || die "Could not set the version in $APP_TOML."
[ "$(tauri_version)" = "$VERSION" ] || die "Could not set the version in $TAURI_CONF."

# --workspace updates only this project's own packages in the lock file; the other crates keep their versions.
(cd core && cargo update --workspace --offline) || die "cargo could not refresh core/Cargo.lock. The edits are in your working tree: undo them with 'git restore .'."
(cd app/src-tauri && cargo update --workspace --offline) || die "cargo could not refresh app/src-tauri/Cargo.lock. The edits are in your working tree: undo them with 'git restore .'."

if printf '%s' "$HEADING" | grep -q 'unreleased'; then
  sed -i.release.bak -E "s/^(## $TAG - )unreleased/\1$TODAY/" CHANGELOG.md
  rm -f CHANGELOG.md.release.bak
fi

# ---------------------------------------------------------------- gates
echo "Running the gates (a few minutes)..."
GATE_FAILED=""
for dir in core app/src-tauri; do
  (cd "$dir" && cargo test) || GATE_FAILED="cargo test in $dir"
  [ -z "$GATE_FAILED" ] || break
  (cd "$dir" && cargo clippy -- -D warnings) || GATE_FAILED="cargo clippy in $dir"
  [ -z "$GATE_FAILED" ] || break
  (cd "$dir" && cargo fmt --check) || GATE_FAILED="cargo fmt --check in $dir"
  [ -z "$GATE_FAILED" ] || break
done
if [ -z "$GATE_FAILED" ]; then
  for script in scripts/release.sh scripts/setup/mac.sh scripts/setup/linux.sh; do
    bash -n "$script" || { GATE_FAILED="bash -n $script"; break; }
  done
fi
if [ -n "$GATE_FAILED" ]; then
  die "A gate failed: $GATE_FAILED. Nothing was committed. Fix it, or undo the version edits with 'git restore .', then run this again."
fi

# ---------------------------------------------------------------- commit and tag
git add "$CORE_TOML" core/Cargo.lock "$APP_TOML" app/src-tauri/Cargo.lock "$TAURI_CONF" CHANGELOG.md
git commit -m "release: $TAG"
git tag -a "$TAG" -m "Meeting Notes $TAG"

echo
echo "Done: $TAG is committed and tagged on this computer. Nothing was pushed."
echo
echo "Push it with:"
echo "  git push origin main $TAG"
echo
echo "Then CI builds the installers. It takes 15 to 25 minutes (GitHub > Actions > Build installers)."
echo "When the build is green the release is public as 'Latest'. People see Update available within a day."
