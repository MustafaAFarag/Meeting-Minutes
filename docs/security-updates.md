# Security updates: what to do when a CVE drops

Users of this app are not technical. They never patch anything themselves: we patch, release, and the widget
offers them **Update available**. This page is the routine for us.

## 1. Does the CVE touch us? (about 10 minutes)

MN has no PHP and no Node.js at runtime. A PHP or Node CVE only matters for our other servers, not this app.
What MN does ship:

| Part | Where the version is set | How to check |
|---|---|---|
| Rust crates (core + app) | `core/Cargo.lock`, `app/src-tauri/Cargo.lock` | `cargo audit` in `core/` and `app/src-tauri/` |
| Tauri + WebView glue (tauri, wry, tao) | `app/src-tauri/Cargo.toml` | `cargo audit`, and the Tauri security advisories on GitHub |
| System WebView | the user's OS (WebView2 on Windows, WKWebView on Mac, WebKitGTK on Linux) | OS updates; nothing to ship |
| whisper.cpp + speech models | the `b5130` tag in `.github/workflows/build.yml` (Mac); URL + SHA-256 constants in the 3 setup scripts (`WHISPER_SHA`, `MODEL_SHA`, `VAD_SHA`) | whisper.cpp releases page. When you change a URL, change its SHA-256 too, or installs stop with "not the expected file" |
| Swift helper | `mac/audiotap/Package.swift` | Swift / macOS security notes |
| GitHub Actions | `uses:` lines in `.github/workflows/build.yml` | the action's releases page |

A crate advisory only needs an urgent release when our code calls the affected API. Check with
`grep -rn "<function name>" core/src app/src-tauri/src`. If we do not call it, bump it in the next normal release.

## 2. Patch (about 15 minutes)

1. Branch: `git switch -c fix/cve-<id>`.
2. Same major version: `cargo update -p <crate>` in both `core/` and `app/src-tauri/`.
   New major version: change the version in that `Cargo.toml`, then `cargo update -p <crate>`.
3. Re-run `cargo audit` in both folders until the advisory is gone.
4. Run the gates in both folders: `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check`.
5. Add the fix to `CHANGELOG.md` under a new `unreleased` heading. In "What's new for you" write one plain line,
   for example "Security fix. Nothing changes in how you use the app."

## 3. Release (about 30 minutes, mostly waiting for CI)

1. Merge to `main`.
2. `bash scripts/release.sh X.Y.Z --dry-run`, then without `--dry-run`.
3. `git push origin main vX.Y.Z`.
4. Wait for Actions > "Build installers" to go green and the release to show as Latest (see `docs/building.md`).
5. Users see **Update available** within a day. For an urgent fix, message them to click **Update** now.

## 4. Something private was pushed (about 15 minutes, then wait for GitHub)

A history rewrite plus force-push is not enough: GitHub keeps serving the old commits by their ID.

1. Rewrite the history and force-push as before.
2. Open a GitHub Support request ("remove sensitive data / cached views") and list the old commit IDs
   (`.git/filter-repo/commit-map`). Ask them to run a garbage collection.
3. Check that it is gone:
   `curl -s -o /dev/null -w "%{http_code}" https://api.github.com/repos/MustafaAFarag/Meeting-Minutes/commits/<old-id>`
   must print `404`.
4. Treat the data as already seen by others. For a token or key, revoke it first.

## 5. Every month (about 10 minutes)

- `cargo audit` in both folders, even when no CVE made the news.
- Read the Tauri, whisper.cpp and GitHub Actions release notes for security fixes.
- The last full audit report is in `~/security-audit-skill/mn/run-<N>/REPORT.md`. Re-run the audit
  (`security audit this codebase`) after big changes: each run adds to the previous one.
