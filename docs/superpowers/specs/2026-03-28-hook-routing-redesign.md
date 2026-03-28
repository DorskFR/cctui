# CCT-22: Hook Routing Redesign — Eliminate Channel HTTP Server

## Problem

Each Claude Code instance spawns its own cctui-channel MCP subprocess. All channels try to bind the same HTTP hook port (8701) configured in `~/.claude/settings.json`. Only the first succeeds. Subsequent channels never receive their SessionStart hook, never register with the server, and are invisible to the TUI. TUI-to-Claude messaging never works because the channel never starts polling.

## Design

Route all hooks through the cctui-server instead of the channel's local HTTP server. The channel becomes a pure stdio-to-HTTP bridge with no port to bind.

### Architecture

```
Before:
  Claude Code → spawns channel (binds :8701)
  settings.json hooks → localhost:8701 (CONFLICT with multiple instances)

After:
  Claude Code → spawns channel (no HTTP server)
  settings.json hooks → cctui-server (:8700 or cctui.dorsk.dev)
  channel ←→ cctui-server (REST polling, no port needed)
```

### Matching Key: (machine_id, ppid)

The channel subprocess doesn't know its Claude session ID at startup — the MCP protocol doesn't provide it. The SessionStart hook has the session_id but doesn't know which channel to deliver it to.

Solution: match them by `(machine_id, ppid)`.

- The channel knows `process.ppid` (its parent = the Claude Code process) and `hostname()` (machine_id)
- The SessionStart hook command runs inside the Claude Code process, so `$PPID` in the hook shell command is the same Claude Code PID
- The composite key `(machine_id, ppid)` uniquely identifies a Claude instance even with multiple sessions on the same machine

### Flow

**1. Channel startup (immediate, before MCP handshake completes):**
```
Channel → POST /api/v1/channels/register
  { machine_id: hostname(), ppid: process.ppid, cwd: process.cwd() }

Server → 201 { channel_id: "uuid" }
  Server stores: pending channel awaiting session assignment
```

**2. Channel polls for session assignment (loop until matched):**
```
Channel → GET /api/v1/channels/{channel_id}/session

Server → 200 { status: "waiting" }         (not yet matched)
Server → 200 { status: "matched", session_id, transcript_path, model }  (matched!)
```

**3. SessionStart hook fires (Claude Code runs this):**

`settings.json` hook command becomes:
```sh
jq -c '. + {ppid: env.PPID}' | curl -sf -X POST https://cctui.dorsk.dev/api/v1/hooks/session-start \
  -H 'Content-Type: application/json' \
  -H 'Authorization: Bearer $CCTUI_AGENT_TOKEN' -d @-
```

This augments the hook payload with `$PPID` before sending to the server.

Server receives: `{ session_id, cwd, model, transcript_path, ppid }` plus machine_id from hostname header or agent token.

Server matches against pending channel registrations by `(machine_id, ppid)`, fills in the session_id on the channel record.

**4. Channel receives session assignment:**

Next poll returns `{ status: "matched", session_id, transcript_path, model }`.

Channel then:
- Registers the session: `POST /api/v1/sessions/register` (upsert)
- Starts polling for pending messages
- Starts tailing the transcript file
- Is fully operational

**5. PreToolUse hook:**

Points directly to the cctui-server. No channel involvement.
```json
{
  "type": "http",
  "url": "https://cctui.dorsk.dev/api/v1/check"
}
```

The payload already includes `session_id`, the server already has the policy evaluation logic.

### Reconnection & Retry

The channel retries at every stage with exponential backoff:

- **Channel registration fails** (server unreachable): retry `POST /channels/register` every 2s, up to 60 attempts. This covers server restart, network blip, or cold start ordering.
- **Session poll returns "waiting" indefinitely** (hook never arrived): keep polling every 2s. If the SessionStart hook failed (network issue), it will fire again on reconnect or can be re-triggered. After 5 minutes with no match, log a warning but keep polling.
- **SessionStart hook fails** (server unreachable at hook time): the hook command itself retries 3 times with 1s sleep. If all fail, the next PreToolUse hook or a manual re-trigger can deliver the session info later. The channel is already polling and will pick it up whenever it arrives.
- **Server restarts mid-session**: channel's message polling and event posting will fail temporarily. Both already retry. The session is upserted on re-registration so state is restored.

### What Gets Deleted

- `channel/src/hooks.ts` — entire file, the channel HTTP server
- Hook port config (`CCTUI_HOOK_PORT` env var, `config.hookPort`)
- Port binding logic in `channel/src/index.ts`

### What Gets Added

**Server side:**
- `POST /api/v1/channels/register` — channel registration endpoint (machine_id, ppid, cwd)
- `GET /api/v1/channels/{channel_id}/session` — session assignment poll
- `POST /api/v1/hooks/session-start` — receives hook payloads, matches to channels
- In-memory pending channel store (HashMap, no DB needed — channels are ephemeral)

**Channel side:**
- Startup: register with server, poll for session assignment
- Remove all HTTP server code
- Remove hook port configuration

**Setup script:**
- `settings.json` hooks point to cctui-server URL instead of localhost:8701
- Hook command augmented with `jq` to inject `$PPID`
- No more `CCTUI_HOOK_PORT`

### Edge Cases

**Two sessions in same cwd on same machine:** Differentiated by ppid — each Claude process has a unique PID.

**Channel starts before server:** Retry loop handles this. Channel keeps trying to register.

**SessionStart hook arrives before channel registers:** Server queues the hook payload. When the channel registers with matching (machine_id, ppid), it's immediately matched.

**Claude session resumes (--resume):** SessionStart fires again with the same session_id. Server upserts. Channel gets re-matched. Transcript tailing resumes from where it left off (file offset tracking).

**Server runs remotely (cctui.dorsk.dev):** All communication is HTTP. No localhost dependency. The only requirement is network reachability from the machine running Claude.
