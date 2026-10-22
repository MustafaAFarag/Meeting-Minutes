#!/usr/bin/env bash
# Builds the universal (arm64 + x86_64) release binary and prints its path on the last line.
set -euo pipefail
cd "$(dirname "$0")"

swift build -c release --arch arm64 --arch x86_64 >&2

# With several --arch flags SwiftPM writes to .build/apple/Products/Release.
BIN_DIR="$(swift build -c release --arch arm64 --arch x86_64 --show-bin-path 2>/dev/null || true)"
BIN="${BIN_DIR:-.build/apple/Products/Release}/mn-audiotap"
[ -x "$BIN" ] || BIN=".build/apple/Products/Release/mn-audiotap"
[ -x "$BIN" ] || { echo "build.sh: mn-audiotap binary not found after build" >&2; exit 1; }

lipo -archs "$BIN" >&2
echo "$(cd "$(dirname "$BIN")" && pwd)/mn-audiotap"
