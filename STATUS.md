# cctui — Project Status

## What We Have (v0.1 — Proof of Concept)

Working end-to-end: Claude sessions register with a central server, tool usage and conversation history are streamed and stored, and a TUI can view sessions and their conversations.

### Server (`cctui-server`)
- Axum HTTP server with PostgreSQL storage
- Session registration via `POST /api/v1/sessions/register` (uses Claude's own session_id)
- PreToolUse HTTP hook captures tool calls (`POST /api/v1/check`)
- Transcript streamer endpoint (`POST /api/v1/events/{session_id}`) ingests full conversation events
- Session list with historical (terminated) sessions from DB
- WebSocket hub for live event fan-out to TUI clients
- Stale session reaper (90s → disconnected, 5m → terminated)
- Bootstrap/setup endpoints that generate local hook scripts
- Bearer token auth (agent + admin roles)

### TUI (`cctui-tui`)
- Full-screen session list grouped by machine name
- Full-screen conversation view with timestamps and basic role coloring
- Session selection fetches conversation history from REST API
- Live events via WebSocket subscription
- Scrolling (j/k/g/G), help overlay (?), basic chat input (i)
- Connects to server via REST + WebSocket

### Channel (`channel/` — Bun/TypeScript)
- MCP channel server spawned by Claude Code via `.mcp.json`
- Bidirectional messaging: TUI → Claude via channel notifications, Claude → TUI via reply tool
- HTTP hook server for SessionStart (session registration) and PreToolUse (policy proxy)
- Transcript JSONL tailer replaces Python streamer
- Connects to cctui-server via REST for event posting and pending message polling

### Agent Integration
- `SessionStart` hook: channel HTTP endpoint receives hook, registers session with server, starts transcript tailing
- `PreToolUse` HTTP hook: channel proxies to server for policy check, caches result
- Transcript tailer: built-in to channel, reads JSONL file, POSTs parsed events to server
- Setup: `make setup/claude` (with server running) configures `.mcp.json` and installs hooks into `~/.claude/settings.json`

### JSONL Archive
- Per-machine backup of `~/.claude/projects/**/*.jsonl` to server PVC
- `archive_index` table tracks sha256 + size + line_count per (machine, session)
- `HEAD /api/v1/archive/{project_dir}/{session_id}?sha256=<hex>` → 204 present / 404 absent
- `PUT /api/v1/archive/{project_dir}/{session_id}` streams to `.partial` + atomic rename, optional `X-CCTUI-SHA256` verify, 100 MiB cap
- Channel triggers: startup scan, periodic while session live (`CCTUI_ARCHIVE_INTERVAL_MINUTES`, default 15), flush on SIGTERM/SIGINT
- Restore = `rsync <pvc>/<machine_uuid>/projects/ ~/.claude/projects/`

### Infrastructure
- Docker Compose for dev/test PostgreSQL
- Makefile with all common targets
- Lefthook pre-commit hooks (fmt, clippy, check, biome)
- K8s manifests + Dockerfile for deployment

---

## What's Missing / Needs Work

### 1. TUI Polish
**Priority: High**

Current TUI is functional but crude:
- **Markdown rendering** — assistant messages contain raw markdown, should render with formatting (bold, code blocks, lists)
- **Clean message display** — strip XML tags (`<local-command-caveat>`, `<command-name>`, etc.), ANSI escape codes, and system noise from user messages
- **Borderless layout** — remove distracting box borders, use spacing and color for structure instead
- **Code block rendering** — syntax highlighting or at least distinct background for code
- **Active session tabs** — sidebar or top tabs showing active sessions as icons/names for quick switching
- **Tool call formatting** — show command in a distinct style, collapse long inputs
- **Better scrolling** — page up/down, mouse scroll support
- **Terminal resize handling** — test and fix layout at different terminal sizes

### 2. Policy Engine
**Priority: Medium**

Evolve from the existing workflow-guard daemon (`infra.dorsk.dev/overlays/ai/claude-worker/files/workflow-guard/daemon.py`):
- Multi-tenant policy enforcement (per-session workflow steps)
- Markdown-driven rules with [allowed]/[disallowed]/[transition] blocks
- The PreToolUse check endpoint already has the hook; just needs policy logic instead of allow-all

### 3. Credential Vault / Account Picker
**Priority: Medium**

- Store API keys in server (Vault-backed in K8s)
- Assign accounts per session on registration
- Support multiple Claude accounts (personal, work, different orgs)

### 4. Prompt & Skill Library
**Priority: Medium**

- Central repository of prompts, skills, CLAUDE.md content
- Push to agents on registration (bootstrap script already has placeholder)
- Version management

### 5. Token/Cost Dashboard
**Priority: Low**

- The data model supports token tracking (TokenUsage struct) but:
  - Token counts aren't populated from transcript events
  - Need to parse usage data from Claude's stream-json format
  - Aggregate dashboards in TUI (per-session, per-machine, total)

### 6. Multi-Machine Deployment
**Priority: Low**

- Currently tested only on localhost
- Need to verify with K8s deployment, Lima VMs, remote machines
- TLS/HTTPS for production
- Agent binary distribution (download from server vs package manager)

---

## Architecture Decisions Made

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Server stack | Rust + Axum + PostgreSQL | Same language as TUI, proven stack |
| Session ID | Use Claude's own session_id | Must match PreToolUse hook's session_id |
| Event streaming | Transcript file tailing → HTTP POST | Simpler than WebSocket from agent side |
| Hook configuration | `~/.claude/settings.json` | Only user-level settings file where hooks work |
| Auth | Bearer tokens | Simple, works with HTTP hooks |
| Session cleanup | Reaper task (no Stop hook) | Claude's Stop hook fires per-turn, not per-session |

## Key Learnings

- Claude Code's `managed-settings.json` lives at `/etc/claude-code/` (Linux), not `~/.claude/`
- `settings.local.json` at `~/.claude/` level is NOT read — it's project-scoped
- Hooks must go in `~/.claude/settings.json` (user settings)
- `SessionStart` hook receives JSON on stdin with `session_id`, `transcript_path`, `cwd`
- `PreToolUse` HTTP hook sends JSON body with `session_id`, `tool_name`, `tool_input`
- The Stop hook fires after EVERY assistant turn, not just session end — don't use for cleanup
- `curl ... | sh` in a hook consumes stdin — the hook input is lost
- Transcript file doesn't exist when SessionStart fires — streamer must wait for it
- `Popen` in a hook script needs `start_new_session=True` to survive after the hook exits
