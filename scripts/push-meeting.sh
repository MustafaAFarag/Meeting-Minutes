#!/usr/bin/env bash
# Usage: push-meeting.sh <session_dir> <card.json> [claude|codex]
# Publishes one meeting (card + transcript) as files of the "My Meetings" artifact,
# plus a rebuilt data/index.json listing every session in the notes dir that has a card.
# Env: MN_ARTIFACT_URL (else artifact/ARTIFACT_URL.txt), MN_CLAUDE_CONFIG_DIR (else normal login).
# Exits non-zero unless the headless publish replies OK.
set -euo pipefail
[ $# -ge 2 ] || { echo "usage: $0 <session_dir> <card.json> [claude|codex]" >&2; exit 2; }
SESSION=$(cd "$1" && pwd); CARD=$(cd "$(dirname "$2")" && pwd)/$(basename "$2"); BY=${3:-claude}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
URL=${MN_ARTIFACT_URL:-$(head -n1 "$ROOT/artifact/ARTIFACT_URL.txt" 2>/dev/null || true)}
[ -n "$URL" ] || { echo "no artifact URL (set MN_ARTIFACT_URL)" >&2; exit 2; }
[ -n "${MN_CLAUDE_CONFIG_DIR:-}" ] && export CLAUDE_CONFIG_DIR="$MN_CLAUDE_CONFIG_DIR"

STAGE=$(mktemp -d); trap 'rm -rf "$STAGE"' EXIT
cp "$ROOT/artifact/index.html" "$STAGE/index.html"
mkdir -p "$STAGE/data/meetings" "$STAGE/data/transcripts"

SID=$(python3 - "$SESSION" "$CARD" "$BY" "$STAGE" <<'PY'
import json, sys, os, re, shutil, datetime
session, card, by, stage = sys.argv[1:5]
notes = os.path.dirname(session.rstrip("/"))
meta = json.load(open(os.path.join(session, "session.json")))
sid = os.path.basename(session.rstrip("/"))
start = datetime.datetime.fromisoformat(meta["started_at"]); end = datetime.datetime.fromisoformat(meta["ended_at"])
if not re.fullmatch(r"\d{4}-\d\d-\d\d_\d{4}", sid):
    sid = start.strftime("%Y-%m-%d_%H%M")
if not os.path.exists(os.path.join(session, "card.json")):
    shutil.copy(card, os.path.join(session, "card.json"))
doc = json.load(open(card))
doc.update(id=sid, started_at=meta["started_at"], ended_at=meta["ended_at"],
           duration_min=round((end - start).total_seconds() / 60), generated_by=by,
           generated_at=datetime.datetime.now().astimezone().isoformat(timespec="seconds"))
def dump(path, obj): json.dump(obj, open(path, "w"), ensure_ascii=False, indent=1)
dump(os.path.join(stage, "data/meetings", sid + ".json"), doc)
dump(os.path.join(session, "meeting.json"), doc)          # remembered for the index
shutil.copy(os.path.join(session, "transcript.md"), os.path.join(stage, "data/transcripts", sid + ".md"))
# index: every session in the notes dir that has a card
entries = []
for d in sorted(os.listdir(notes)):
    p = os.path.join(notes, d)
    try:
        m = json.load(open(os.path.join(p, "meeting.json"))) if os.path.exists(os.path.join(p, "meeting.json")) else None
        if m is None and os.path.exists(os.path.join(p, "card.json")):
            c = json.load(open(os.path.join(p, "card.json"))); s = json.load(open(os.path.join(p, "session.json")))
            m = {"id": d, "title": c.get("title"), "started_at": s["started_at"]}
    except (OSError, ValueError, KeyError):
        continue
    if m: entries.append({k: m.get(k) for k in ("id", "title", "started_at", "generated_at") if m.get(k)})
entries.sort(key=lambda e: e["started_at"], reverse=True)
dump(os.path.join(stage, "data/index.json"),
     {"updated_at": datetime.datetime.now().astimezone().isoformat(timespec="seconds"), "meetings": entries})
print(sid)
PY
)

FILES="{\"data/index.json\":\"data/index.json\",\"data/meetings/$SID.json\":\"data/meetings/$SID.json\",\"data/transcripts/$SID.md\":\"data/transcripts/$SID.md\"}"
PROMPT="Update the existing artifact $URL: publish with url set to it, file_path ./index.html (unchanged, do not edit it, omit capabilities so the stored ones are kept), and files $FILES. If the publish is refused because the artifact was not read, read it with action read and then publish again exactly the same way. Reply with only OK plus the list of published files, or the exact error."
RES=$(cd "$STAGE" && CLAUDE_CODE_ARTIFACT=1 claude -p "$PROMPT" --allowedTools Artifact < /dev/null) || { echo "claude failed: $RES" >&2; exit 1; }
echo "$RES"
case "$RES" in OK*) ;; *) echo "publish failed" >&2; exit 1;; esac
