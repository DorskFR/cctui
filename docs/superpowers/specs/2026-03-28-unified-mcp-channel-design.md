# CCT-16: Unified MCP Channel Server

## Overview

Replace three fragile components (`bootstrap.sh.tpl`, `streamer.py`, `cctui-shim`) with a single TypeScript/Bun MCP channel server that provides full bidirectional communication between Claude Code sessions and the cctui ecosystem.

## Architecture

Claude Code spawns `cctui-channel` via `.mcp.json` as an MCP subprocess (stdio transport). The channel server bridges three interfaces:

```
Claude Code (MCP stdio)
    |
cctui-channel (Bun/TS)
    |--- MCP stdio <-> Claude Code
    |    - IN:  notifications/claude/channel (push TUI messages to Claude)
    |    - OUT: reply tool (Claude sends messages back to TUI)
    |
    |--- HTTP :8701 <- Claude Code hooks
    |    - POST /hooks/session-start  (SessionStart hook)
    |    - POST /hooks/pre-tool-use   (PreToolUse hook, proxied to server)
    |
    |--- WebSocket -> cctui-server :8700
         - Session registration
         - Transcript event streaming
         - Pending message subscription (TUI -> Claude)
         - Reply forwarding (Claude -> TUI)
```

## Components

### 1. MCP Server (`src/mcp.ts`)

Standard MCP server using `@modelcontextprotocol/sdk` with:

- **Capability:** `experimental: { 'claude/channel': {} }` — registers the notification listener
- **Capability:** `tools: {}` — enables reply tool discovery
- **Instructions:** System prompt text telling Claude that messages arrive as `<channel source="cctui" ...>` tags and to reply using the `reply` tool

**Reply tool schema:**
```typescript
{
  name: 'reply',
  description: 'Send a message back to the TUI operator',
  inputSchema: {
    type: 'object',
    properties: {
      text: { type: 'string', description: 'The message to send' },
    },
    required: ['text'],
  },
}
```

When Claude calls `reply`, the channel server forwards the message to cctui-server as an `AgentEvent::Reply`.

**Inbound notifications:** When the channel server receives a pending message from cctui-server, it calls:
```typescript
await mcp.notification({
  method: 'notifications/claude/channel',
  params: {
    content: messageContent,
    meta: { sender: 'tui', message_id: uuid },
  },
})
```

Claude sees: `<channel source="cctui" sender="tui" message_id="...">message text</channel>`

### 2. HTTP Hook Server (`src/hooks.ts`)

Bun HTTP server on configurable port (default 8701, via `CCTUI_HOOK_PORT`).

**`POST /hooks/session-start`**
- Receives Claude's SessionStart JSON from stdin (piped via curl in the hook command)
- Payload: `{ session_id, transcript_path, cwd, ... }`
- Actions:
  1. Store session_id and transcript_path in channel state
  2. Register session with cctui-server via WebSocket or REST
  3. Start transcript file tailer
- Response: `200 OK`

**`POST /hooks/pre-tool-use`**
- Receives Claude's PreToolUse JSON body
- Payload: `{ session_id, tool_name, tool_input }`
- Actions:
  1. Proxy to cctui-server `POST /api/v1/check`
  2. Return the policy verdict to Claude Code
- Response: `{ "decision": "allow" | "deny", "reason": "..." }`

### 3. WebSocket Bridge (`src/bridge.ts`)

Persistent WebSocket connection to cctui-server at `ws://{CCTUI_URL}/api/v1/stream/{session_id}`.

**Upstream (channel -> server):**
- `AgentEvent::Text` — assistant messages from transcript
- `AgentEvent::ToolCall` — tool calls from transcript
- `AgentEvent::ToolResult` — tool results from transcript
- `AgentEvent::Reply` — Claude's reply tool responses (new variant)
- `AgentEvent::Heartbeat` — periodic heartbeat with token usage

**Downstream (server -> channel):**
- `PendingMessage { content }` — TUI messages queued for this session (new)
- Channel server converts these to MCP channel notifications

**Reconnection:** Auto-reconnect with exponential backoff (1s, 2s, 4s, ... max 30s). Buffer events during disconnect.

### 4. Transcript Tailer (`src/transcript.ts`)

Replaces `streamer.py`. Watches and tails the Claude transcript JSONL file.

**Behavior:**
- Waits for transcript file to appear (it doesn't exist at SessionStart time)
- Uses `Bun.file().watch()` or `fs.watch` for file change notifications
- Reads new lines as they're appended
- Parses each JSON line and categorizes:
  - `type: "human"` → `AgentEvent::Text` with role indicator
  - `type: "assistant"` with `content[].type: "text"` → `AgentEvent::Text`
  - `type: "assistant"` with `content[].type: "tool_use"` → `AgentEvent::ToolCall`
  - `type: "assistant"` with `content[].type: "tool_result"` → `AgentEvent::ToolResult`
- Extracts `usage` fields for token tracking → `AgentEvent::Heartbeat`
- Sends parsed events to cctui-server via the WebSocket bridge

### 5. Entry Point (`src/index.ts`)

Orchestrates startup:
1. Read config from environment (`CCTUI_URL`, `CCTUI_AGENT_TOKEN`, `CCTUI_HOOK_PORT`)
2. Create MCP server and connect via stdio transport
3. Start HTTP hook server
4. Wait for SessionStart hook to provide session_id
5. Connect WebSocket to cctui-server
6. Start transcript tailer (once transcript_path is known)

## Server-Side Changes (cctui-server)

### New Proto Types (`cctui-proto`)

```rust
// In AgentEvent enum
Reply { content: String, ts: i64 },

// New message type for server -> agent WebSocket
pub enum ServerToAgent {
    PendingMessage { id: Uuid, content: String },
}
```

### Agent WebSocket Becomes Bidirectional

Currently `handle_agent_stream()` only reads from the agent. Change to:
- **Read:** `AgentEvent` from channel (unchanged)
- **Write:** `ServerToAgent::PendingMessage` when TUI queues a message for this session

When `registry.queue_message()` is called, broadcast to the agent's WebSocket connection in addition to storing in the pending queue.

### TUI Rendering

`AgentEvent::Reply` rendered in conversation view as a distinct message type — similar to assistant text but marked as a channel reply (e.g., different prefix or color).

## Hook Configuration

### `.mcp.json` (generated by setup)

```json
{
  "mcpServers": {
    "cctui": {
      "command": "bun",
      "args": ["run", "{CHANNEL_DIR}/src/index.ts"],
      "env": {
        "CCTUI_URL": "http://localhost:8700",
        "CCTUI_AGENT_TOKEN": "dev-agent",
        "CCTUI_HOOK_PORT": "8701"
      }
    }
  }
}
```

### `~/.claude/settings.json` hooks

```json
{
  "hooks": {
    "SessionStart": [{
      "type": "command",
      "command": "curl -sf -X POST http://localhost:8701/hooks/session-start -H 'Content-Type: application/json' -d @-"
    }],
    "PreToolUse": [{
      "type": "http",
      "url": "http://localhost:8701/hooks/pre-tool-use"
    }]
  }
}
```

## What Gets Retired

| Component | Replacement |
|-----------|-------------|
| `crates/cctui-shim/` | Deleted from workspace entirely |
| `scripts/bootstrap.sh.tpl` | SessionStart hook → curl to channel HTTP |
| `scripts/streamer.py` | `src/transcript.ts` in channel |
| `scripts/setup.py.tpl` | Updated to generate `.mcp.json` + simplified hooks |

## File Structure

```
channel/
  package.json            # bun, @modelcontextprotocol/sdk, zod
  tsconfig.json
  src/
    index.ts              # Entry point: orchestrates startup
    mcp.ts                # MCP server (channel capability, reply tool)
    hooks.ts              # HTTP server for SessionStart/PreToolUse
    bridge.ts             # WebSocket client to cctui-server
    transcript.ts         # JSONL transcript file tailer
    types.ts              # Shared types
```

## Testing

- **Unit:** Mock MCP transport, test notification formatting and reply tool handling
- **Integration:** Start channel + cctui-server, simulate SessionStart hook, send messages both directions
- **E2E:** Full flow with Claude Code using `--dangerously-load-development-channels`

## Open Questions (Resolved)

- **Port conflicts with multiple sessions:** Use configurable `CCTUI_HOOK_PORT`. For multi-session on one machine, each `.mcp.json` entry gets a different port.
- **Language choice:** TypeScript/Bun — native MCP SDK, fast I/O, matches channels spec.
