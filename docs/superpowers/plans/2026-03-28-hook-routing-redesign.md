# CCT-22: Hook Routing Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate the channel's HTTP hook server and route all hooks through the cctui-server, matching channels to sessions by (machine_id, ppid).

**Architecture:** Each channel subprocess registers with the cctui-server on startup using (machine_id, ppid). The SessionStart hook posts directly to the server with ppid injected. The server matches them and the channel polls until matched. No port binding, no conflicts.

**Tech Stack:** Rust/Axum (server), TypeScript/Bun (channel), Python (setup script)

---

## File Structure

### Server (Rust)
- **Create:** `crates/cctui-server/src/routes/channels.rs` — channel register + session poll + hook receiver endpoints
- **Modify:** `crates/cctui-server/src/routes/mod.rs` — add `pub mod channels;`
- **Modify:** `crates/cctui-server/src/main.rs` — add new routes, add channel store to state
- **Modify:** `crates/cctui-server/src/state.rs` — add `SharedChannelStore` to `AppState`

### Channel (TypeScript)
- **Modify:** `channel/src/bridge.ts` — add `registerChannel()` and `pollSession()` methods
- **Modify:** `channel/src/index.ts` — rewrite startup: register channel, poll for session, remove hook server
- **Modify:** `channel/src/types.ts` — remove `hookPort` from Config, add channel registration types
- **Delete:** `channel/src/hooks.ts` — entire file

### Setup
- **Modify:** `scripts/setup.py.tpl` — hooks point to server, inject $PPID, remove HOOK_PORT
- **Modify:** `scripts/setup-claude.sh` — remove HOOK_PORT parameter
- **Modify:** `Makefile` — remove HOOK_PORT references

### Tests
- **Modify:** `channel/test/bridge.test.ts` — add tests for new bridge methods
- **Delete:** `channel/test/hooks.test.ts` — hooks.ts is deleted
- **Modify:** `crates/cctui-server/src/routes/channels.rs` — inline Rust tests

---

### Task 1: Server — Channel Store and Registration Endpoint

**Files:**
- Create: `crates/cctui-server/src/routes/channels.rs`
- Modify: `crates/cctui-server/src/routes/mod.rs`
- Modify: `crates/cctui-server/src/state.rs`
- Modify: `crates/cctui-server/src/main.rs`

- [ ] **Step 1: Write the channel store and types in channels.rs**

```rust
// crates/cctui-server/src/routes/channels.rs
use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::state::AppState;

// --- Channel store (in-memory, channels are ephemeral) ---

#[derive(Debug, Clone)]
pub struct PendingChannel {
    pub channel_id: String,
    pub machine_id: String,
    pub ppid: u32,
    pub cwd: String,
    pub registered_at: DateTime<Utc>,
    /// Filled in when SessionStart hook matches this channel
    pub session_info: Option<SessionAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAssignment {
    pub session_id: String,
    pub transcript_path: String,
    pub model: String,
}

pub type SharedChannelStore = Arc<RwLock<ChannelStore>>;

pub struct ChannelStore {
    /// Channels indexed by channel_id
    channels: HashMap<String, PendingChannel>,
    /// Unmatched hook payloads waiting for a channel to claim them
    pending_hooks: Vec<HookPayload>,
}

#[derive(Debug, Clone)]
struct HookPayload {
    pub machine_id: String,
    pub ppid: u32,
    pub session_id: String,
    pub transcript_path: String,
    pub model: String,
    pub received_at: DateTime<Utc>,
}

impl ChannelStore {
    pub fn new() -> Self {
        Self { channels: HashMap::new(), pending_hooks: Vec::new() }
    }

    pub fn shared() -> SharedChannelStore {
        Arc::new(RwLock::new(Self::new()))
    }

    /// Register a new channel. If a matching hook payload already arrived, match immediately.
    pub fn register_channel(&mut self, machine_id: String, ppid: u32, cwd: String) -> String {
        let channel_id = Uuid::new_v4().to_string();

        // Check if a hook payload already arrived for this (machine_id, ppid)
        let assignment = self.try_match_hook(&machine_id, ppid);

        self.channels.insert(
            channel_id.clone(),
            PendingChannel {
                channel_id: channel_id.clone(),
                machine_id,
                ppid,
                cwd,
                registered_at: Utc::now(),
                session_info: assignment,
            },
        );
        channel_id
    }

    /// Get session assignment for a channel. Returns None if not yet matched.
    pub fn get_assignment(&self, channel_id: &str) -> Option<&SessionAssignment> {
        self.channels.get(channel_id).and_then(|c| c.session_info.as_ref())
    }

    /// Receive a SessionStart hook payload. Try to match to a pending channel.
    pub fn receive_hook(&mut self, machine_id: String, ppid: u32, session_id: String, transcript_path: String, model: String) {
        // Try to match to an existing channel
        let matched = self.channels.values_mut().find(|c| {
            c.machine_id == machine_id && c.ppid == ppid && c.session_info.is_none()
        });

        if let Some(channel) = matched {
            channel.session_info = Some(SessionAssignment {
                session_id,
                transcript_path,
                model,
            });
        } else {
            // No channel registered yet — queue for later matching
            self.pending_hooks.push(HookPayload {
                machine_id,
                ppid,
                session_id,
                transcript_path,
                model,
                received_at: Utc::now(),
            });
        }
    }

    /// Try to match a channel against queued hook payloads
    fn try_match_hook(&mut self, machine_id: &str, ppid: u32) -> Option<SessionAssignment> {
        let idx = self.pending_hooks.iter().position(|h| h.machine_id == machine_id && h.ppid == ppid)?;
        let hook = self.pending_hooks.remove(idx);
        Some(SessionAssignment {
            session_id: hook.session_id,
            transcript_path: hook.transcript_path,
            model: hook.model,
        })
    }

    /// Clean up channels older than max_age_secs
    pub fn reap_stale(&mut self, max_age_secs: i64) {
        let cutoff = Utc::now() - chrono::Duration::seconds(max_age_secs);
        self.channels.retain(|_, c| c.registered_at > cutoff);
        self.pending_hooks.retain(|h| h.received_at > cutoff);
    }
}
```

- [ ] **Step 2: Add the route handlers in channels.rs**

Append to the same file:

```rust
// --- Request/Response types ---

#[derive(Debug, Deserialize)]
pub struct RegisterChannelRequest {
    pub machine_id: String,
    pub ppid: u32,
    pub cwd: String,
}

#[derive(Debug, Serialize)]
pub struct RegisterChannelResponse {
    pub channel_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionPollResponse {
    Waiting,
    Matched {
        session_id: String,
        transcript_path: String,
        model: String,
    },
}

#[derive(Debug, Deserialize)]
pub struct SessionStartHookPayload {
    pub session_id: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    #[serde(default)]
    pub ppid: Option<u32>,
    /// machine_id sent by the hook script
    #[serde(default)]
    pub machine_id: Option<String>,
}

// --- Handlers ---

pub async fn register_channel(
    State(state): State<AppState>,
    Json(req): Json<RegisterChannelRequest>,
) -> (StatusCode, Json<RegisterChannelResponse>) {
    let channel_id = state.channel_store.write().await.register_channel(
        req.machine_id,
        req.ppid,
        req.cwd,
    );
    tracing::info!(channel_id = %channel_id, "channel registered");
    (StatusCode::CREATED, Json(RegisterChannelResponse { channel_id }))
}

pub async fn poll_session(
    State(state): State<AppState>,
    axum::extract::Path(channel_id): axum::extract::Path<String>,
) -> Json<SessionPollResponse> {
    let store = state.channel_store.read().await;
    match store.get_assignment(&channel_id) {
        Some(assignment) => Json(SessionPollResponse::Matched {
            session_id: assignment.session_id.clone(),
            transcript_path: assignment.transcript_path.clone(),
            model: assignment.model.clone(),
        }),
        None => Json(SessionPollResponse::Waiting),
    }
}

pub async fn session_start_hook(
    State(state): State<AppState>,
    Json(req): Json<SessionStartHookPayload>,
) -> StatusCode {
    let machine_id = req.machine_id.unwrap_or_default();
    let ppid = req.ppid.unwrap_or(0);

    tracing::info!(
        session_id = %req.session_id,
        machine_id = %machine_id,
        ppid = ppid,
        "SessionStart hook received"
    );

    if ppid == 0 || machine_id.is_empty() {
        tracing::warn!("SessionStart hook missing ppid or machine_id, cannot match to channel");
        return StatusCode::BAD_REQUEST;
    }

    state.channel_store.write().await.receive_hook(
        machine_id,
        ppid,
        req.session_id,
        req.transcript_path.unwrap_or_default(),
        req.model.unwrap_or_default(),
    );

    StatusCode::OK
}

// --- Tests ---

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_hook_matches() {
        let mut store = ChannelStore::new();
        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        assert!(store.get_assignment(&cid).is_none());

        store.receive_hook("machine-1".into(), 1234, "session-abc".into(), "/path/t.jsonl".into(), "opus".into());
        let assignment = store.get_assignment(&cid).unwrap();
        assert_eq!(assignment.session_id, "session-abc");
        assert_eq!(assignment.transcript_path, "/path/t.jsonl");
    }

    #[test]
    fn hook_then_register_matches() {
        let mut store = ChannelStore::new();
        store.receive_hook("machine-1".into(), 1234, "session-abc".into(), "/path/t.jsonl".into(), "opus".into());

        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        let assignment = store.get_assignment(&cid).unwrap();
        assert_eq!(assignment.session_id, "session-abc");
    }

    #[test]
    fn different_ppid_no_match() {
        let mut store = ChannelStore::new();
        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        store.receive_hook("machine-1".into(), 5678, "session-other".into(), "/path".into(), "".into());
        assert!(store.get_assignment(&cid).is_none());
    }

    #[test]
    fn different_machine_no_match() {
        let mut store = ChannelStore::new();
        let cid = store.register_channel("machine-1".into(), 1234, "/tmp".into());
        store.receive_hook("machine-2".into(), 1234, "session-other".into(), "/path".into(), "".into());
        assert!(store.get_assignment(&cid).is_none());
    }

    #[test]
    fn multiple_channels_match_correctly() {
        let mut store = ChannelStore::new();
        let cid1 = store.register_channel("m".into(), 100, "/a".into());
        let cid2 = store.register_channel("m".into(), 200, "/b".into());

        store.receive_hook("m".into(), 200, "session-b".into(), "/b.jsonl".into(), "".into());
        store.receive_hook("m".into(), 100, "session-a".into(), "/a.jsonl".into(), "".into());

        assert_eq!(store.get_assignment(&cid1).unwrap().session_id, "session-a");
        assert_eq!(store.get_assignment(&cid2).unwrap().session_id, "session-b");
    }

    #[test]
    fn reap_stale_removes_old_entries() {
        let mut store = ChannelStore::new();
        store.register_channel("m".into(), 100, "/tmp".into());
        assert_eq!(store.channels.len(), 1);
        // With max_age 0, everything is stale
        store.reap_stale(0);
        assert_eq!(store.channels.len(), 0);
    }
}
```

- [ ] **Step 3: Add channel_store to AppState**

In `crates/cctui-server/src/state.rs`:

```rust
use crate::auth::AuthConfig;
use crate::config::Config;
use crate::registry::SharedRegistry;
use crate::routes::channels::SharedChannelStore;
use sqlx::PgPool;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub registry: SharedRegistry,
    pub channel_store: SharedChannelStore,
    #[allow(dead_code)]
    pub auth_config: AuthConfig,
}
```

- [ ] **Step 4: Wire up routes and state in main.rs**

In `crates/cctui-server/src/main.rs`, add to the state construction:

```rust
let state = AppState {
    pool,
    config: config.clone(),
    registry: Registry::shared(),
    channel_store: routes::channels::ChannelStore::shared(),
    auth_config: auth_config.clone(),
};
```

Add routes to `mod.rs`:
```rust
pub mod channels;
```

Add to the unauthenticated app router (alongside `/health`, `/api/v1/check`, etc.) since the channel uses agent token in its own headers:

```rust
.route("/api/v1/channels/register", post(routes::channels::register_channel))
.route("/api/v1/channels/{channel_id}/session", get(routes::channels::poll_session))
.route("/api/v1/hooks/session-start", post(routes::channels::session_start_hook))
```

Add channel store reaping to the existing `reaper_task`:

```rust
// Inside the reaper loop, after session reaping:
{
    let mut store = state.channel_store.write().await;
    store.reap_stale(600); // 10 minutes
}
```

- [ ] **Step 5: Run tests and verify**

Run: `cargo test --package cctui-server`
Expected: All existing tests pass + new channel store tests pass.

Run: `cargo clippy --workspace`
Expected: Clean.

- [ ] **Step 6: Commit**

```bash
git add crates/cctui-server/src/routes/channels.rs crates/cctui-server/src/routes/mod.rs crates/cctui-server/src/state.rs crates/cctui-server/src/main.rs
git commit --no-gpg-sign -m "feat(server): channel store and hook routing endpoints (CCT-22)"
```

---

### Task 2: Channel — Add Bridge Methods for Channel Registration

**Files:**
- Modify: `channel/src/bridge.ts`
- Modify: `channel/src/types.ts`
- Modify: `channel/test/bridge.test.ts`

- [ ] **Step 1: Update types.ts — remove hookPort, add channel types**

```typescript
// channel/src/types.ts
/** Configuration from environment variables. */
export interface Config {
  /** cctui-server base URL, e.g. "http://localhost:8700" */
  serverUrl: string;
  /** Bearer token for agent auth */
  agentToken: string;
}

export function loadConfig(): Config {
  return {
    serverUrl: process.env.CCTUI_URL ?? "http://localhost:8700",
    agentToken: process.env.CCTUI_AGENT_TOKEN ?? "dev-agent",
  };
}

/** Payload from Claude Code's SessionStart hook stdin. */
export interface SessionStartPayload {
  session_id: string;
  cwd: string;
  model?: string;
  transcript_path?: string;
}

/** Payload from Claude Code's PreToolUse hook. */
export interface PreToolUsePayload {
  session_id: string;
  tool_name: string;
  tool_input: Record<string, unknown>;
}

/** Event sent to cctui-server's POST /api/v1/events/{session_id} endpoint. */
export interface StreamerEvent {
  session_id: string;
  type: string;
  content?: string;
  tool?: string;
  input?: Record<string, unknown>;
  tool_use_id?: string;
  ts: number;
  tokens_in?: number;
  tokens_out?: number;
  cost_usd?: number;
}

/** Pending message received from cctui-server. */
export interface PendingMessage {
  id: string;
  content: string;
  created_at: string;
}

/** Session state held by the channel after session assignment. */
export interface SessionState {
  sessionId: string;
  transcriptPath: string | null;
  cwd: string;
  machineId: string;
  model: string;
}

/** Response from POST /api/v1/channels/register */
export interface ChannelRegisterResponse {
  channel_id: string;
}

/** Response from GET /api/v1/channels/{channel_id}/session */
export type SessionPollResponse =
  | { status: "waiting" }
  | { status: "matched"; session_id: string; transcript_path: string; model: string };
```

- [ ] **Step 2: Add registerChannel and pollSession to bridge.ts**

Remove the `import type { PolicyVerdict } from "./hooks";` line (hooks.ts will be deleted).

Add a standalone `PolicyVerdict` type and two new methods to `ServerBridge`:

```typescript
// At the top of bridge.ts, replace the PolicyVerdict import:
export interface PolicyVerdict {
  decision: "allow" | "deny";
  reason?: string;
}

// Add these imports:
import type { StreamerEvent, PreToolUsePayload, PendingMessage, ChannelRegisterResponse, SessionPollResponse } from "./types";

// Add to the ServerBridge class:

async registerChannel(machineId: string, ppid: number, cwd: string): Promise<ChannelRegisterResponse> {
  const res = await fetch(`${this.baseUrl}/api/v1/channels/register`, {
    method: "POST",
    headers: this.headers(),
    body: JSON.stringify({ machine_id: machineId, ppid, cwd }),
  });
  if (!res.ok) {
    throw new Error(`channel register failed: ${res.status} ${await res.text()}`);
  }
  return res.json();
}

async pollSession(channelId: string): Promise<SessionPollResponse> {
  const res = await fetch(`${this.baseUrl}/api/v1/channels/${channelId}/session`, {
    headers: this.headers(),
  });
  if (!res.ok) {
    throw new Error(`poll session failed: ${res.status}`);
  }
  return res.json();
}
```

- [ ] **Step 3: Write tests for new bridge methods**

Add to `channel/test/bridge.test.ts`:

```typescript
test("registerChannel sends correct POST with machine_id and ppid", async () => {
  let capturedUrl = "";
  let capturedBody = "";
  globalThis.fetch = mock(async (input: string | URL | Request, init?: RequestInit) => {
    capturedUrl = input.toString();
    capturedBody = init?.body as string;
    return new Response(
      JSON.stringify({ channel_id: "ch-123" }),
      { status: 201, headers: { "Content-Type": "application/json" } },
    );
  }) as typeof fetch;

  const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
  const result = await bridge.registerChannel("my-machine", 1234, "/home/user/project");
  expect(capturedUrl).toBe("http://localhost:8700/api/v1/channels/register");
  expect(result.channel_id).toBe("ch-123");
  const body = JSON.parse(capturedBody);
  expect(body.machine_id).toBe("my-machine");
  expect(body.ppid).toBe(1234);
  expect(body.cwd).toBe("/home/user/project");
});

test("pollSession returns waiting status", async () => {
  globalThis.fetch = mock(async () => {
    return new Response(JSON.stringify({ status: "waiting" }), {
      status: 200, headers: { "Content-Type": "application/json" },
    });
  }) as typeof fetch;

  const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
  const result = await bridge.pollSession("ch-123");
  expect(result.status).toBe("waiting");
});

test("pollSession returns matched status with session info", async () => {
  globalThis.fetch = mock(async () => {
    return new Response(
      JSON.stringify({ status: "matched", session_id: "sess-abc", transcript_path: "/tmp/t.jsonl", model: "opus" }),
      { status: 200, headers: { "Content-Type": "application/json" } },
    );
  }) as typeof fetch;

  const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
  const result = await bridge.pollSession("ch-123");
  expect(result.status).toBe("matched");
  if (result.status === "matched") {
    expect(result.session_id).toBe("sess-abc");
    expect(result.transcript_path).toBe("/tmp/t.jsonl");
  }
});

test("registerChannel throws on failure", async () => {
  globalThis.fetch = mock(async () => new Response("error", { status: 500 })) as typeof fetch;
  const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
  expect(bridge.registerChannel("m", 1, "/")).rejects.toThrow("channel register failed");
});
```

- [ ] **Step 4: Run tests**

Run: `cd channel && bun test`
Expected: All tests pass including new ones.

- [ ] **Step 5: Commit**

```bash
git add channel/src/bridge.ts channel/src/types.ts channel/test/bridge.test.ts
git commit --no-gpg-sign -m "feat(channel): bridge methods for channel registration (CCT-22)"
```

---

### Task 3: Channel — Rewrite Startup to Use Server-Side Matching

**Files:**
- Modify: `channel/src/index.ts`
- Delete: `channel/src/hooks.ts`
- Delete: `channel/test/hooks.test.ts`

- [ ] **Step 1: Rewrite index.ts — remove hook server, add channel registration loop**

Replace the entire file:

```typescript
#!/usr/bin/env bun
import { loadConfig } from "./types";
import type { SessionState } from "./types";
import { createChannelServer } from "./mcp";
import { ServerBridge } from "./bridge";
import { tailTranscript } from "./transcript";
import { hostname } from "os";
import { basename } from "path";

const config = loadConfig();
const bridge = new ServerBridge(config.serverUrl, config.agentToken);

let session: SessionState | null = null;
let tailAbort: AbortController | null = null;

// --- MCP channel server (stdio) ---
const { pushMessage, connect } = createChannelServer({
  onReply: async (text) => {
    if (!session) return;
    await bridge.postEvent(session.sessionId, {
      session_id: session.sessionId,
      type: "assistant_message",
      content: `[Reply to TUI] ${text}`,
      ts: Math.floor(Date.now() / 1000),
    });
  },
});

// --- Bridge: poll for pending TUI messages, push as MCP notifications ---
bridge.onPendingMessage = (msg) => {
  console.error(`[cctui-channel] received pending message: ${msg.content.slice(0, 100)}`);
  pushMessage(msg.content, { message_id: msg.id });
};

// --- Channel registration and session discovery ---
async function registerAndWaitForSession(): Promise<void> {
  const machineId = hostname();
  const cwd = process.cwd();
  const ppid = process.ppid;

  // Step 1: Register channel with retry
  let channelId: string | null = null;
  for (let attempt = 1; attempt <= 60; attempt++) {
    try {
      const res = await bridge.registerChannel(machineId, ppid, cwd);
      channelId = res.channel_id;
      console.error(`[cctui-channel] channel registered: ${channelId} (machine=${machineId}, ppid=${ppid})`);
      break;
    } catch (err) {
      console.error(`[cctui-channel] registration attempt ${attempt}/60 failed:`, err);
      await Bun.sleep(2000);
    }
  }

  if (!channelId) {
    console.error("[cctui-channel] failed to register channel after 60 attempts");
    return;
  }

  // Step 2: Poll for session assignment
  console.error("[cctui-channel] waiting for SessionStart hook to match...");
  let matched = false;
  for (let attempt = 1; !matched; attempt++) {
    try {
      const poll = await bridge.pollSession(channelId);
      if (poll.status === "matched") {
        console.error(`[cctui-channel] session matched: ${poll.session_id}`);

        session = {
          sessionId: poll.session_id,
          transcriptPath: poll.transcript_path || null,
          cwd,
          machineId,
          model: poll.model || "",
        };

        // Register the session with the server (upsert)
        await bridge.registerSession({
          claude_session_id: poll.session_id,
          machine_id: machineId,
          working_dir: cwd,
          metadata: {
            project_name: basename(cwd),
            model: poll.model || "",
            transcript_path: poll.transcript_path || "",
          },
        });
        console.error(`[cctui-channel] session registered with server: ${poll.session_id}`);

        // Start polling for pending messages
        bridge.startPolling(poll.session_id);

        // Start tailing transcript
        if (session.transcriptPath) {
          tailAbort = new AbortController();
          tailTranscript(
            poll.session_id,
            session.transcriptPath,
            (event) => bridge.postEvent(poll.session_id, event),
            tailAbort.signal,
          );
        }

        matched = true;
      }
    } catch (err) {
      // Poll failed — server might be down, just retry
    }

    if (!matched) {
      if (attempt % 150 === 0) {
        console.error(`[cctui-channel] still waiting for session match (${attempt * 2}s elapsed)...`);
      }
      await Bun.sleep(2000);
    }
  }
}

// --- Start ---
// Connect MCP first (stdio handshake), then register with server
await connect();
console.error("[cctui-channel] connected to Claude Code");

// Start registration in the background (don't block MCP)
registerAndWaitForSession();

process.on("SIGTERM", () => {
  tailAbort?.abort();
  bridge.stopPolling();
  process.exit(0);
});

process.on("SIGINT", () => {
  tailAbort?.abort();
  bridge.stopPolling();
  process.exit(0);
});
```

- [ ] **Step 2: Delete hooks.ts and hooks.test.ts**

```bash
rm channel/src/hooks.ts channel/test/hooks.test.ts
```

- [ ] **Step 3: Run tests**

Run: `cd channel && bun test`
Expected: All remaining tests pass. hooks.test.ts is gone.

Run: `bunx tsc --noEmit`
Expected: Clean.

- [ ] **Step 4: Commit**

```bash
git add channel/src/index.ts channel/src/types.ts
git rm channel/src/hooks.ts channel/test/hooks.test.ts
git commit --no-gpg-sign -m "feat(channel): replace hook server with server-side session matching (CCT-22)"
```

---

### Task 4: Setup Script — Route Hooks to Server

**Files:**
- Modify: `scripts/setup.py.tpl`
- Modify: `scripts/setup-claude.sh`
- Modify: `Makefile`

- [ ] **Step 1: Update setup.py.tpl**

Replace the entire file:

```python
#!/usr/bin/env python3
"""Setup script generated by cctui-server. Configures Claude Code to use cctui-channel."""
import json
import os
import shutil

SERVER_URL = os.environ.get("__SERVER_URL__", "__SERVER_URL__")
TOKEN = os.environ.get("__TOKEN__", "__TOKEN__")

# --- Find channel directory ---
channel_dir = os.environ.get(
    "CCTUI_CHANNEL_DIR",
    os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "channel"),
)

if not os.path.isdir(channel_dir):
    print(f"[cctui] ERROR: channel dir not found at {channel_dir}")
    print("        Set CCTUI_CHANNEL_DIR to the absolute path of the channel/ directory")
    exit(1)

# --- Write MCP server config to ~/.claude.json ---
claude_json_path = os.path.expanduser("~/.claude.json")

mcp_entry = {
    "command": shutil.which("bun") or os.path.expanduser("~/.bun/bin/bun"),
    "args": ["run", os.path.join(channel_dir, "src", "index.ts")],
    "env": {
        "CCTUI_URL": SERVER_URL,
        "CCTUI_AGENT_TOKEN": TOKEN,
    },
}

if os.path.exists(claude_json_path):
    with open(claude_json_path) as f:
        claude_config = json.load(f)
else:
    claude_config = {}

claude_config.setdefault("mcpServers", {})
claude_config["mcpServers"]["cctui"] = mcp_entry

with open(claude_json_path, "w") as f:
    json.dump(claude_config, f, indent=2)

print(f"[cctui] wrote MCP server config to {claude_json_path}")

# --- Merge hooks into settings.json ---
settings_path = os.path.expanduser("~/.claude/settings.json")

# SessionStart: pipe stdin through jq to inject $PPID and machine hostname, then POST to server
session_start_cmd = (
    f'jq -c --arg ppid "$PPID" --arg mid "$(hostname)" '
    f"'. + {{ppid: ($ppid | tonumber), machine_id: $mid}}' "
    f'| curl -sf -X POST {SERVER_URL}/api/v1/hooks/session-start '
    f"-H 'Content-Type: application/json' "
    f"-H 'Authorization: Bearer {TOKEN}' -d @-"
)

hooks = {
    "SessionStart": [{
        "hooks": [{
            "type": "command",
            "command": session_start_cmd,
        }],
    }],
    "PreToolUse": [{
        "hooks": [{
            "type": "http",
            "url": f"{SERVER_URL}/api/v1/check",
        }],
    }],
}

if os.path.exists(settings_path):
    with open(settings_path) as f:
        settings = json.load(f)
    print(f"[cctui] merging hooks into {settings_path}")
else:
    settings = {}
    print(f"[cctui] creating {settings_path}")

settings["hooks"] = hooks

os.makedirs(os.path.dirname(settings_path), exist_ok=True)
with open(settings_path, "w") as f:
    json.dump(settings, f, indent=2)

print(f"[cctui] done - hooks route to {SERVER_URL}")
print()
print("[cctui] IMPORTANT: To enable TUI→Claude messaging, start Claude with:")
print("        claude --dangerously-load-development-channels server:cctui")
print("        Without this flag, Claude sees the tools but won't receive channel messages.")
```

- [ ] **Step 2: Update setup-claude.sh — remove HOOK_PORT**

```bash
#!/bin/sh
# Configure Claude Code to use the cctui-channel MCP server.
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "==> Installing channel dependencies..."
(cd "$REPO_DIR/channel" && bun install --silent)

echo "==> Configuring Claude Code hooks and MCP server..."
CCTUI_CHANNEL_DIR="$REPO_DIR/channel" \
  __SERVER_URL__="$URL" \
  __TOKEN__="$TOKEN" \
  python3 "$SCRIPT_DIR/setup.py.tpl"

echo ""
echo "==> Done! Next time you start Claude Code, the cctui-channel will activate."
echo "    Server: $URL"
```

- [ ] **Step 3: Update Makefile — remove HOOK_PORT**

Remove the `CCTUI_HOOK_PORT` references. Change setup/claude:

```makefile
setup/claude:  ## Configure local Claude Code to use cctui-channel
	./scripts/setup-claude.sh $(CCTUI_URL) dev-agent
```

- [ ] **Step 4: Commit**

```bash
git add scripts/setup.py.tpl scripts/setup-claude.sh Makefile
git commit --no-gpg-sign -m "feat(setup): route hooks to server, remove hook port (CCT-22)"
```

---

### Task 5: Run Full Test Suite and Verify

**Files:** None (verification only)

- [ ] **Step 1: Run all Rust tests**

Run: `cargo test --workspace`
Expected: All tests pass including new channel store tests.

- [ ] **Step 2: Run all channel tests**

Run: `cd channel && bun test`
Expected: All tests pass. hooks tests are gone.

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace`
Expected: Clean.

- [ ] **Step 4: Run TypeScript check**

Run: `bunx tsc --noEmit`
Expected: Clean.

- [ ] **Step 5: Run setup script to update local config**

Run: `make setup/claude`
Expected: settings.json updated with new hook commands pointing to server.

- [ ] **Step 6: Commit any remaining fixes**

If any tests or lints failed, fix and commit.
