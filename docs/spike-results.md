# Spike results (2026-10-22, Arch Linux, Claude Code 2.1.308, codex-cli 0.163.0)

| Check | Result |
|---|---|
| (a) `claude -p` publishes an artifact | Works only with env `CLAUDE_CODE_ARTIFACT=1` plus `--allowedTools Artifact`. The env var is undocumented. Test page published (private) (source `docs/spike-page.html`) |
| (b) claude.ai connectors in `claude -p` | They load. On the CTCHealth account, M365 `outlook_calendar_search` returned today's 5 events. On the personal account, M365 shows `needs-auth` |
| (c) `codex exec` on ChatGPT login | Works (`codex exec --skip-git-repo-check "..." < /dev/null`) |
| (d) PipeWire capture | `<default-sink>.monitor` recorded a test tone (max -4.8 dB). Default source recorded the mic. A real Teams web call is not tested yet |

Notes
- In `-p`, pipe stdin from `/dev/null` or Claude waits 3 s for stdin.
- `--allowedTools` must name each MCP tool, for example `mcp__claude_ai_Microsoft_365__outlook_calendar_search`.
