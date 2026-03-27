# Unified MCP Channel Server Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace bootstrap.sh, streamer.py, and cctui-shim with a single Bun/TypeScript MCP channel server that provides full bidirectional communication between Claude Code and the cctui ecosystem.

**Architecture:** A Bun process spawned by Claude Code via `.mcp.json` that (1) acts as an MCP channel server over stdio for bidirectional messaging, (2) runs an HTTP server for Claude Code hooks (SessionStart, PreToolUse), and (3) connects to cctui-server via WebSocket + REST for event streaming and message relay.

**Tech Stack:** Bun, TypeScript, `@modelcontextprotocol/sdk`, `zod`

---

## File Structure

```
channel/
  package.json              # bun, @modelcontextprotocol/sdk, zod
  tsconfig.json             # strict TS config
  biome.json                # formatting config (match project)
  src/
    index.ts                # Entry point: orchestrates startup, wires components
    mcp.ts                  # MCP server: channel capability, reply tool, notification push
    hooks.ts                # HTTP server: SessionStart + PreToolUse hook endpoints
    bridge.ts               # WebSocket client to cctui-server + REST helpers
    transcript.ts           # JSONL transcript file tailer + parser
    types.ts                # Shared types (hook payloads, agent events, config)
  test/
    transcript.test.ts      # Transcript parsing tests
    hooks.test.ts           # Hook endpoint tests
    mcp.test.ts             # MCP notification + reply tool tests
    bridge.test.ts          # WebSocket bridge tests
```

---

### Task 1: Retire cctui-shim and scaffold channel directory

**Files:**
- Delete: `crates/cctui-shim/` (entire directory)
- Modify: `Cargo.toml:3-8` (remove shim from workspace members)
- Modify: `Makefile:19,60-61` (remove shim references)
- Create: `channel/package.json`
- Create: `channel/tsconfig.json`
- Create: `channel/src/types.ts`

- [ ] **Step 1: Delete cctui-shim from workspace**

Remove the `crates/cctui-shim` directory entirely:

```bash
rm -rf crates/cctui-shim
```

- [ ] **Step 2: Remove shim from Cargo.toml workspace members**

In `Cargo.toml`, change the workspace members from:

```toml
members = [
    "crates/cctui-proto",
    "crates/cctui-server",
    "crates/cctui-tui",
    "crates/cctui-shim",
]
```

to:

```toml
members = [
    "crates/cctui-proto",
    "crates/cctui-server",
    "crates/cctui-tui",
]
```

- [ ] **Step 3: Remove shim references from Makefile**

In `Makefile`, remove the `run/shim` phony target from line 19:

Change `.PHONY: run/server run/tui run/shim` to `.PHONY: run/server run/tui`

Remove lines 60-61:

```makefile
run/shim:  ## Run the shim (pipe stdin to server WS)
	cargo run -p cctui-shim -- relay --session-id $(SESSION_ID) --ws-url $(WS_URL)
```

Add a new channel target:

```makefile
run/channel:  ## Install and run the channel server (for development)
	cd channel && bun install && bun run src/index.ts
```

- [ ] **Step 4: Verify Rust workspace still builds**

```bash
cargo check --workspace
```

Expected: builds successfully without cctui-shim.

- [ ] **Step 5: Create channel/package.json**

```json
{
  "name": "cctui-channel",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "start": "bun run src/index.ts",
    "test": "bun test"
  },
  "dependencies": {
    "@modelcontextprotocol/sdk": "^1.12.1",
    "zod": "^3.24.4"
  },
  "devDependencies": {
    "@types/bun": "latest",
    "typescript": "^5.8.3"
  }
}
```

- [ ] **Step 6: Create channel/tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "outDir": "dist",
    "rootDir": "src",
    "types": ["bun"]
  },
  "include": ["src/**/*.ts", "test/**/*.ts"]
}
```

- [ ] **Step 7: Create channel/src/types.ts**

```typescript
/** Configuration from environment variables. */
export interface Config {
  /** cctui-server base URL, e.g. "http://localhost:8700" */
  serverUrl: string;
  /** Bearer token for agent auth */
  agentToken: string;
  /** Port for the local HTTP hook server */
  hookPort: number;
}

export function loadConfig(): Config {
  return {
    serverUrl: process.env.CCTUI_URL ?? "http://localhost:8700",
    agentToken: process.env.CCTUI_AGENT_TOKEN ?? "dev-agent",
    hookPort: Number(process.env.CCTUI_HOOK_PORT ?? "8701"),
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

/** Session state held by the channel after SessionStart fires. */
export interface SessionState {
  sessionId: string;
  transcriptPath: string | null;
  cwd: string;
  machineId: string;
  model: string;
}
```

- [ ] **Step 8: Install dependencies**

```bash
cd channel && bun install
```

- [ ] **Step 9: Commit**

```bash
git add -A && git commit --no-gpg-sign -m "feat(channel): scaffold Bun/TS channel, retire cctui-shim (CCT-16)"
```

---

### Task 2: Transcript parser (port from streamer.py)

**Files:**
- Create: `channel/src/transcript.ts`
- Create: `channel/test/transcript.test.ts`

- [ ] **Step 1: Write failing tests for transcript line parsing**

Create `channel/test/transcript.test.ts`:

```typescript
import { describe, expect, test } from "bun:test";
import { parseLine, parseUsage } from "../src/transcript";

describe("parseLine", () => {
  test("parses user text message", () => {
    const line = JSON.stringify({
      type: "human",
      message: { role: "user", content: "hello world" },
    });
    expect(parseLine(line)).toEqual({
      type: "user_message",
      content: "hello world",
    });
  });

  test("parses user message with content list containing text", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [{ type: "text", text: "some input" }],
      },
    });
    expect(parseLine(line)).toEqual({
      type: "user_message",
      content: "some input",
    });
  });

  test("parses user message with tool_result", () => {
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "abc123",
            content: "result text",
          },
        ],
      },
    });
    expect(parseLine(line)).toEqual({
      type: "tool_result",
      tool_use_id: "abc123",
      content: "result text",
    });
  });

  test("parses assistant text message", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: {
        role: "assistant",
        content: [{ type: "text", text: "I will help" }],
      },
    });
    expect(parseLine(line)).toEqual({
      type: "assistant_message",
      content: "I will help",
    });
  });

  test("parses assistant tool_use", () => {
    const line = JSON.stringify({
      type: "assistant",
      message: {
        role: "assistant",
        content: [
          { type: "tool_use", name: "Bash", input: { command: "ls" } },
        ],
      },
    });
    expect(parseLine(line)).toEqual({
      type: "tool_call",
      tool: "Bash",
      input: { command: "ls" },
    });
  });

  test("returns null for system messages", () => {
    const line = JSON.stringify({
      type: "system",
      message: { role: "system", content: "system prompt" },
    });
    expect(parseLine(line)).toBeNull();
  });

  test("returns null for file-history-snapshot", () => {
    const line = JSON.stringify({ type: "file-history-snapshot" });
    expect(parseLine(line)).toBeNull();
  });

  test("returns null for invalid JSON", () => {
    expect(parseLine("not json")).toBeNull();
  });

  test("truncates long tool_result content to 500 chars", () => {
    const longContent = "x".repeat(600);
    const line = JSON.stringify({
      type: "human",
      message: {
        role: "user",
        content: [
          { type: "tool_result", tool_use_id: "id1", content: longContent },
        ],
      },
    });
    const result = parseLine(line);
    expect(result?.content?.length).toBe(500);
  });
});

describe("parseUsage", () => {
  test("parses usage from message.usage", () => {
    const line = JSON.stringify({
      message: {
        role: "assistant",
        usage: { input_tokens: 1000, output_tokens: 500 },
      },
    });
    const result = parseUsage(line);
    expect(result).not.toBeNull();
    expect(result!.tokens_in).toBe(1000);
    expect(result!.tokens_out).toBe(500);
    expect(result!.cost_usd).toBeCloseTo(0.0105);
  });

  test("includes cache tokens in input count", () => {
    const line = JSON.stringify({
      message: {
        role: "assistant",
        usage: {
          input_tokens: 100,
          cache_creation_input_tokens: 200,
          cache_read_input_tokens: 300,
          output_tokens: 50,
        },
      },
    });
    const result = parseUsage(line);
    expect(result!.tokens_in).toBe(600);
    expect(result!.tokens_out).toBe(50);
  });

  test("returns null when no usage data", () => {
    const line = JSON.stringify({
      message: { role: "user", content: "hi" },
    });
    expect(parseUsage(line)).toBeNull();
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd channel && bun test test/transcript.test.ts
```

Expected: FAIL — module `../src/transcript` not found.

- [ ] **Step 3: Implement transcript parser**

Create `channel/src/transcript.ts`:

```typescript
import type { StreamerEvent } from "./types";

interface ParsedEvent {
  type: string;
  content?: string;
  tool?: string;
  input?: Record<string, unknown>;
  tool_use_id?: string;
}

const SKIP_TYPES = new Set([
  "file-history-snapshot",
  "queue-operation",
  "system",
]);

export function parseLine(line: string): ParsedEvent | null {
  let d: Record<string, unknown>;
  try {
    d = JSON.parse(line);
  } catch {
    return null;
  }

  const msgType = (d.type as string) ?? "";
  if (SKIP_TYPES.has(msgType)) return null;

  const msg = (d.message as Record<string, unknown>) ?? {};
  const role = (msg.role as string) ?? "";
  const content = msg.content;

  if (role === "system") return null;

  if (role === "user") {
    if (typeof content === "string" && content) {
      return { type: "user_message", content };
    }
    if (Array.isArray(content)) {
      for (const part of content) {
        if (part.type === "tool_result") {
          const raw = String(part.content ?? "");
          return {
            type: "tool_result",
            tool_use_id: part.tool_use_id ?? "",
            content: raw.slice(0, 500),
          };
        }
        if (part.type === "text") {
          return { type: "user_message", content: part.text ?? "" };
        }
      }
    }
    return null;
  }

  if (role === "assistant") {
    if (Array.isArray(content)) {
      for (const part of content) {
        if (part.type === "text") {
          return { type: "assistant_message", content: part.text ?? "" };
        }
        if (part.type === "tool_use") {
          return {
            type: "tool_call",
            tool: part.name ?? "",
            input: part.input ?? {},
          };
        }
      }
    } else if (typeof content === "string" && content) {
      return { type: "assistant_message", content };
    }
    return null;
  }

  return null;
}

interface UsageData {
  tokens_in: number;
  tokens_out: number;
  cost_usd: number;
}

export function parseUsage(line: string): UsageData | null {
  let d: Record<string, unknown>;
  try {
    d = JSON.parse(line);
  } catch {
    return null;
  }

  const msg = (d.message as Record<string, unknown>) ?? {};
  const usage =
    (msg.usage as Record<string, number>) ??
    (d.usage as Record<string, number>);
  if (!usage) return null;

  const tokensIn =
    (usage.input_tokens ?? 0) +
    (usage.cache_creation_input_tokens ?? 0) +
    (usage.cache_read_input_tokens ?? 0);
  const tokensOut = usage.output_tokens ?? 0;
  const costUsd = (tokensIn / 1_000_000) * 3.0 + (tokensOut / 1_000_000) * 15.0;

  return { tokens_in: tokensIn, tokens_out: tokensOut, cost_usd: costUsd };
}

/** Callback for each parsed event from the transcript. */
export type EventCallback = (event: StreamerEvent) => void;

/**
 * Tail a Claude transcript JSONL file, calling `onEvent` for each parsed event.
 * Waits for the file to appear, processes existing content, then watches for changes.
 */
export async function tailTranscript(
  sessionId: string,
  transcriptPath: string,
  onEvent: EventCallback,
  signal?: AbortSignal,
): Promise<void> {
  // Wait up to 30s for file to appear
  for (let i = 0; i < 60; i++) {
    if (signal?.aborted) return;
    const exists = await Bun.file(transcriptPath).exists();
    if (exists) break;
    if (i === 59) return; // file never appeared
    await Bun.sleep(500);
  }

  let offset = 0;

  const processNewContent = async () => {
    const file = Bun.file(transcriptPath);
    const size = file.size;
    if (size <= offset) return;

    const content = await file.text();
    const newContent = content.slice(offset);
    offset = content.length;

    for (const line of newContent.split("\n")) {
      const trimmed = line.trim();
      if (!trimmed) continue;

      const ts = Math.floor(Date.now() / 1000);

      const event = parseLine(trimmed);
      if (event) {
        onEvent({
          session_id: sessionId,
          type: event.type,
          content: event.content,
          tool: event.tool,
          input: event.input,
          tool_use_id: event.tool_use_id,
          ts,
        });
      }

      const usage = parseUsage(trimmed);
      if (usage) {
        onEvent({
          session_id: sessionId,
          type: "usage",
          ts,
          tokens_in: usage.tokens_in,
          tokens_out: usage.tokens_out,
          cost_usd: usage.cost_usd,
        });
      }
    }
  };

  // Process existing content
  await processNewContent();

  // Watch for changes
  const watcher = Bun.file(transcriptPath);
  // Use polling since fs.watch can be unreliable for appended files
  while (!signal?.aborted) {
    await Bun.sleep(300);
    await processNewContent();
  }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd channel && bun test test/transcript.test.ts
```

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
cd channel && git add -A && git commit --no-gpg-sign -m "feat(channel): transcript JSONL parser ported from streamer.py (CCT-16)"
```

---

### Task 3: HTTP hook server

**Files:**
- Create: `channel/src/hooks.ts`
- Create: `channel/test/hooks.test.ts`

- [ ] **Step 1: Write failing tests for hook endpoints**

Create `channel/test/hooks.test.ts`:

```typescript
import { describe, expect, test, mock } from "bun:test";
import { createHookServer } from "../src/hooks";
import type { SessionStartPayload } from "../src/types";

describe("hook server", () => {
  test("POST /hooks/session-start stores session state", async () => {
    const onSessionStart = mock((_payload: SessionStartPayload, _machineId: string) => {});
    const server = createHookServer({ port: 0, onSessionStart, onPreToolUse: async () => ({ decision: "allow" }) });

    const payload: SessionStartPayload = {
      session_id: "abc-123",
      cwd: "/home/user/project",
      model: "opus",
      transcript_path: "/tmp/transcript.jsonl",
    };

    const res = await fetch(`http://localhost:${server.port}/hooks/session-start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(payload),
    });

    expect(res.status).toBe(200);
    expect(onSessionStart).toHaveBeenCalledTimes(1);
    const call = onSessionStart.mock.calls[0];
    expect(call[0].session_id).toBe("abc-123");

    server.stop();
  });

  test("POST /hooks/pre-tool-use proxies to callback", async () => {
    const onPreToolUse = mock(async () => ({ decision: "allow" as const }));
    const server = createHookServer({ port: 0, onSessionStart: () => {}, onPreToolUse });

    const res = await fetch(`http://localhost:${server.port}/hooks/pre-tool-use`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        session_id: "abc-123",
        tool_name: "Bash",
        tool_input: { command: "ls" },
      }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.decision).toBe("allow");
    expect(onPreToolUse).toHaveBeenCalledTimes(1);

    server.stop();
  });

  test("GET /health returns 200", async () => {
    const server = createHookServer({
      port: 0,
      onSessionStart: () => {},
      onPreToolUse: async () => ({ decision: "allow" }),
    });

    const res = await fetch(`http://localhost:${server.port}/health`);
    expect(res.status).toBe(200);

    server.stop();
  });

  test("unknown route returns 404", async () => {
    const server = createHookServer({
      port: 0,
      onSessionStart: () => {},
      onPreToolUse: async () => ({ decision: "allow" }),
    });

    const res = await fetch(`http://localhost:${server.port}/unknown`);
    expect(res.status).toBe(404);

    server.stop();
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd channel && bun test test/hooks.test.ts
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement hook server**

Create `channel/src/hooks.ts`:

```typescript
import type { SessionStartPayload, PreToolUsePayload } from "./types";
import { hostname } from "os";

export interface PolicyVerdict {
  decision: "allow" | "deny";
  reason?: string;
}

export interface HookServerOptions {
  port: number;
  onSessionStart: (payload: SessionStartPayload, machineId: string) => void;
  onPreToolUse: (payload: PreToolUsePayload) => Promise<PolicyVerdict>;
}

export function createHookServer(options: HookServerOptions) {
  const { onSessionStart, onPreToolUse } = options;
  const machineId = hostname();

  const server = Bun.serve({
    port: options.port,
    hostname: "127.0.0.1",
    async fetch(req) {
      const url = new URL(req.url);

      if (url.pathname === "/health" && req.method === "GET") {
        return new Response("ok");
      }

      if (url.pathname === "/hooks/session-start" && req.method === "POST") {
        try {
          const payload: SessionStartPayload = await req.json();
          onSessionStart(payload, machineId);
          return Response.json({ status: "ok" });
        } catch (err) {
          return Response.json(
            { error: String(err) },
            { status: 400 },
          );
        }
      }

      if (url.pathname === "/hooks/pre-tool-use" && req.method === "POST") {
        try {
          const payload: PreToolUsePayload = await req.json();
          const verdict = await onPreToolUse(payload);
          return Response.json(verdict);
        } catch (err) {
          // On error, allow (fail open to avoid blocking Claude)
          return Response.json({ decision: "allow" });
        }
      }

      return new Response("not found", { status: 404 });
    },
  });

  return server;
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd channel && bun test test/hooks.test.ts
```

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
cd channel && git add -A && git commit --no-gpg-sign -m "feat(channel): HTTP hook server for SessionStart + PreToolUse (CCT-16)"
```

---

### Task 4: Bridge to cctui-server (REST + WebSocket)

**Files:**
- Create: `channel/src/bridge.ts`
- Create: `channel/test/bridge.test.ts`

- [ ] **Step 1: Write failing tests for bridge REST methods**

Create `channel/test/bridge.test.ts`:

```typescript
import { describe, expect, test, mock, beforeEach, afterEach } from "bun:test";
import { ServerBridge } from "../src/bridge";

// We test the REST methods by mocking global fetch
describe("ServerBridge", () => {
  let originalFetch: typeof globalThis.fetch;

  beforeEach(() => {
    originalFetch = globalThis.fetch;
  });

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  test("registerSession sends correct POST", async () => {
    let capturedUrl = "";
    let capturedBody = "";
    globalThis.fetch = mock(async (input: string | URL | Request, init?: RequestInit) => {
      capturedUrl = input.toString();
      capturedBody = init?.body as string;
      return new Response(
        JSON.stringify({ session_id: "abc", ws_url: "ws://localhost:8700/api/v1/stream/abc" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const result = await bridge.registerSession({
      claude_session_id: "abc",
      machine_id: "test-host",
      working_dir: "/tmp",
      metadata: { model: "opus" },
    });

    expect(capturedUrl).toBe("http://localhost:8700/api/v1/sessions/register");
    expect(result.session_id).toBe("abc");
    const body = JSON.parse(capturedBody);
    expect(body.machine_id).toBe("test-host");
  });

  test("postEvent sends event to correct URL", async () => {
    let capturedUrl = "";
    globalThis.fetch = mock(async (input: string | URL | Request) => {
      capturedUrl = input.toString();
      return new Response("", { status: 200 });
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    await bridge.postEvent("session-1", {
      session_id: "session-1",
      type: "assistant_message",
      content: "hello",
      ts: 123,
    });

    expect(capturedUrl).toBe("http://localhost:8700/api/v1/events/session-1");
  });

  test("checkPolicy sends tool call and returns verdict", async () => {
    globalThis.fetch = mock(async () => {
      return new Response(
        JSON.stringify({ decision: "allow" }),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const result = await bridge.checkPolicy({
      session_id: "s1",
      tool_name: "Bash",
      tool_input: { command: "ls" },
    });

    expect(result.decision).toBe("allow");
  });

  test("fetchPendingMessages returns messages array", async () => {
    globalThis.fetch = mock(async () => {
      return new Response(
        JSON.stringify([{ id: "msg-1", content: "do this", created_at: "2026-01-01T00:00:00Z" }]),
        { status: 200, headers: { "Content-Type": "application/json" } },
      );
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const msgs = await bridge.fetchPendingMessages("session-1");
    expect(msgs).toHaveLength(1);
    expect(msgs[0].content).toBe("do this");
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd channel && bun test test/bridge.test.ts
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement bridge**

Create `channel/src/bridge.ts`:

```typescript
import type {
  StreamerEvent,
  PreToolUsePayload,
  PendingMessage,
} from "./types";
import type { PolicyVerdict } from "./hooks";

interface RegisterRequest {
  claude_session_id: string;
  machine_id: string;
  working_dir: string;
  parent_session_id?: string;
  metadata?: Record<string, unknown>;
}

interface RegisterResponse {
  session_id: string;
  ws_url: string;
}

export class ServerBridge {
  private baseUrl: string;
  private token: string;
  private pollInterval: ReturnType<typeof setInterval> | null = null;
  public onPendingMessage: ((msg: PendingMessage) => void) | null = null;

  constructor(baseUrl: string, token: string) {
    this.baseUrl = baseUrl;
    this.token = token;
  }

  private headers(): Record<string, string> {
    return {
      "Content-Type": "application/json",
      Authorization: `Bearer ${this.token}`,
    };
  }

  async registerSession(req: RegisterRequest): Promise<RegisterResponse> {
    const res = await fetch(`${this.baseUrl}/api/v1/sessions/register`, {
      method: "POST",
      headers: this.headers(),
      body: JSON.stringify(req),
    });
    if (!res.ok) {
      throw new Error(`register failed: ${res.status} ${await res.text()}`);
    }
    return res.json();
  }

  async postEvent(sessionId: string, event: StreamerEvent): Promise<void> {
    try {
      await fetch(`${this.baseUrl}/api/v1/events/${sessionId}`, {
        method: "POST",
        headers: this.headers(),
        body: JSON.stringify(event),
      });
    } catch {
      // Swallow errors to avoid blocking the tailer
    }
  }

  async checkPolicy(payload: PreToolUsePayload): Promise<PolicyVerdict> {
    try {
      const res = await fetch(`${this.baseUrl}/api/v1/check`, {
        method: "POST",
        headers: this.headers(),
        body: JSON.stringify(payload),
      });
      if (!res.ok) return { decision: "allow" };
      return res.json();
    } catch {
      return { decision: "allow" }; // fail open
    }
  }

  async fetchPendingMessages(sessionId: string): Promise<PendingMessage[]> {
    try {
      const res = await fetch(
        `${this.baseUrl}/api/v1/sessions/${sessionId}/messages/pending`,
        { headers: this.headers() },
      );
      if (!res.ok) return [];
      return res.json();
    } catch {
      return [];
    }
  }

  /**
   * Start polling for pending messages from the TUI.
   * Calls `onPendingMessage` for each message received.
   */
  startPolling(sessionId: string, intervalMs = 1000): void {
    this.stopPolling();
    this.pollInterval = setInterval(async () => {
      const msgs = await this.fetchPendingMessages(sessionId);
      for (const msg of msgs) {
        this.onPendingMessage?.(msg);
      }
    }, intervalMs);
  }

  stopPolling(): void {
    if (this.pollInterval) {
      clearInterval(this.pollInterval);
      this.pollInterval = null;
    }
  }
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd channel && bun test test/bridge.test.ts
```

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
cd channel && git add -A && git commit --no-gpg-sign -m "feat(channel): REST bridge to cctui-server with polling (CCT-16)"
```

---

### Task 5: MCP server (channel capability + reply tool)

**Files:**
- Create: `channel/src/mcp.ts`
- Create: `channel/test/mcp.test.ts`

- [ ] **Step 1: Write failing tests for MCP notification and reply tool**

Create `channel/test/mcp.test.ts`:

```typescript
import { describe, expect, test, mock } from "bun:test";
import { createChannelServer } from "../src/mcp";

describe("createChannelServer", () => {
  test("creates server with channel capability", () => {
    const { server } = createChannelServer({
      onReply: mock(async () => {}),
    });
    // Server should be created without errors
    expect(server).toBeDefined();
  });

  test("pushMessage formats notification correctly", async () => {
    const { pushMessage, server } = createChannelServer({
      onReply: mock(async () => {}),
    });

    // pushMessage should not throw before connection
    // (it will warn but not crash — we test the format)
    expect(typeof pushMessage).toBe("function");
  });
});
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cd channel && bun test test/mcp.test.ts
```

Expected: FAIL — module not found.

- [ ] **Step 3: Implement MCP channel server**

Create `channel/src/mcp.ts`:

```typescript
import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  ListToolsRequestSchema,
  CallToolRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";

export interface ChannelServerOptions {
  onReply: (text: string) => Promise<void>;
}

export function createChannelServer(options: ChannelServerOptions) {
  const { onReply } = options;

  const server = new Server(
    { name: "cctui", version: "0.1.0" },
    {
      capabilities: {
        experimental: { "claude/channel": {} },
        tools: {},
      },
      instructions: [
        'Messages from the TUI operator arrive as <channel source="cctui" sender="tui">.',
        "These are instructions or questions from the human monitoring your session.",
        "Read them carefully and act on them. Reply using the cctui_reply tool to send a response back to the TUI.",
        "Always acknowledge TUI messages, even if briefly.",
      ].join(" "),
    },
  );

  // Tool discovery
  server.setRequestHandler(ListToolsRequestSchema, async () => ({
    tools: [
      {
        name: "cctui_reply",
        description:
          "Send a message back to the TUI operator who is monitoring this session",
        inputSchema: {
          type: "object" as const,
          properties: {
            text: {
              type: "string",
              description: "The message to send to the TUI operator",
            },
          },
          required: ["text"],
        },
      },
    ],
  }));

  // Tool execution
  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    if (req.params.name === "cctui_reply") {
      const { text } = req.params.arguments as { text: string };
      await onReply(text);
      return { content: [{ type: "text" as const, text: "Message sent to TUI." }] };
    }
    throw new Error(`unknown tool: ${req.params.name}`);
  });

  /**
   * Push a message from the TUI into the Claude session as a channel notification.
   */
  async function pushMessage(content: string, meta?: Record<string, string>) {
    try {
      await server.notification({
        method: "notifications/claude/channel",
        params: {
          content,
          meta: { sender: "tui", ...meta },
        },
      });
    } catch (err) {
      // May fail if not yet connected — log but don't crash
      console.error("[cctui-channel] failed to push notification:", err);
    }
  }

  /**
   * Connect to Claude Code over stdio.
   */
  async function connect() {
    const transport = new StdioServerTransport();
    await server.connect(transport);
  }

  return { server, pushMessage, connect };
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd channel && bun test test/mcp.test.ts
```

Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
cd channel && git add -A && git commit --no-gpg-sign -m "feat(channel): MCP server with channel capability and reply tool (CCT-16)"
```

---

### Task 6: Entry point — wire everything together

**Files:**
- Create: `channel/src/index.ts`

- [ ] **Step 1: Implement the entry point**

Create `channel/src/index.ts`:

```typescript
#!/usr/bin/env bun
import { loadConfig } from "./types";
import type { SessionStartPayload, PreToolUsePayload, SessionState } from "./types";
import { createChannelServer } from "./mcp";
import { createHookServer } from "./hooks";
import { ServerBridge } from "./bridge";
import { tailTranscript } from "./transcript";
import { hostname } from "os";
import { basename } from "path";
import { execSync } from "child_process";

const config = loadConfig();
const bridge = new ServerBridge(config.serverUrl, config.agentToken);

let session: SessionState | null = null;
let tailAbort: AbortController | null = null;

// --- MCP channel server (stdio) ---
const { pushMessage, connect } = createChannelServer({
  onReply: async (text) => {
    if (!session) return;
    // Post reply as an agent event so it appears in the TUI conversation
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
  pushMessage(msg.content, { message_id: msg.id });
};

// --- Hook handlers ---
function onSessionStart(payload: SessionStartPayload, machineId: string) {
  const cwd = payload.cwd || process.cwd();
  let gitBranch = "none";
  try {
    gitBranch = execSync(`git -C ${cwd} rev-parse --abbrev-ref HEAD`, {
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();
  } catch {}

  session = {
    sessionId: payload.session_id,
    transcriptPath: payload.transcript_path ?? null,
    cwd,
    machineId,
    model: payload.model ?? "",
  };

  // Register with cctui-server
  bridge
    .registerSession({
      claude_session_id: payload.session_id,
      machine_id: machineId,
      working_dir: cwd,
      metadata: {
        git_branch: gitBranch,
        project_name: basename(cwd),
        model: payload.model ?? "",
        transcript_path: payload.transcript_path ?? "",
      },
    })
    .then(() => {
      console.error(`[cctui-channel] session registered: ${payload.session_id}`);

      // Start polling for TUI messages
      bridge.startPolling(payload.session_id);

      // Start transcript tailer
      if (session?.transcriptPath) {
        tailAbort = new AbortController();
        tailTranscript(
          payload.session_id,
          session.transcriptPath,
          (event) => bridge.postEvent(payload.session_id, event),
          tailAbort.signal,
        );
      }
    })
    .catch((err) => {
      console.error("[cctui-channel] registration failed:", err);
    });
}

async function onPreToolUse(payload: PreToolUsePayload) {
  return bridge.checkPolicy(payload);
}

// --- Start ---
// 1. Connect MCP server (stdio — must happen first, Claude Code is waiting)
await connect();

// 2. Start HTTP hook server
const hookServer = createHookServer({
  port: config.hookPort,
  onSessionStart,
  onPreToolUse,
});

console.error(`[cctui-channel] hook server listening on :${hookServer.port}`);
console.error(`[cctui-channel] connected to Claude Code, waiting for SessionStart hook...`);

// Cleanup on exit
process.on("SIGTERM", () => {
  tailAbort?.abort();
  bridge.stopPolling();
  hookServer.stop();
  process.exit(0);
});

process.on("SIGINT", () => {
  tailAbort?.abort();
  bridge.stopPolling();
  hookServer.stop();
  process.exit(0);
});
```

- [ ] **Step 2: Verify the channel starts without errors (dry run)**

```bash
cd channel && bun check src/index.ts
```

Or just type-check:

```bash
cd channel && bunx tsc --noEmit
```

Expected: no type errors.

- [ ] **Step 3: Commit**

```bash
cd channel && git add -A && git commit --no-gpg-sign -m "feat(channel): entry point wiring MCP + hooks + bridge + transcript (CCT-16)"
```

---

### Task 7: Server-side — add Reply event variant and update event type match

**Files:**
- Modify: `crates/cctui-proto/src/ws.rs:6-13` (add Reply variant to AgentEvent)
- Modify: `crates/cctui-server/src/ws.rs:66-71` (add Reply to event_type match)

- [ ] **Step 1: Add Reply variant to AgentEvent**

In `crates/cctui-proto/src/ws.rs`, add `Reply` to the `AgentEvent` enum. Change:

```rust
pub enum AgentEvent {
    Text { content: String, ts: i64 },
    ToolCall { tool: String, input: serde_json::Value, ts: i64 },
    ToolResult { tool: String, output_summary: String, ts: i64 },
    Heartbeat { tokens_in: u64, tokens_out: u64, cost_usd: f64, ts: i64 },
}
```

to:

```rust
pub enum AgentEvent {
    Text { content: String, ts: i64 },
    ToolCall { tool: String, input: serde_json::Value, ts: i64 },
    ToolResult { tool: String, output_summary: String, ts: i64 },
    Heartbeat { tokens_in: u64, tokens_out: u64, cost_usd: f64, ts: i64 },
    Reply { content: String, ts: i64 },
}
```

- [ ] **Step 2: Update event_type match in ws.rs store_and_broadcast**

In `crates/cctui-server/src/ws.rs`, update the `event_type` match at line 66-71. Change:

```rust
    let event_type = match &event {
        AgentEvent::Text { .. } => "text",
        AgentEvent::ToolCall { .. } => "tool_call",
        AgentEvent::ToolResult { .. } => "tool_result",
        AgentEvent::Heartbeat { .. } => "heartbeat",
    };
```

to:

```rust
    let event_type = match &event {
        AgentEvent::Text { .. } => "text",
        AgentEvent::ToolCall { .. } => "tool_call",
        AgentEvent::ToolResult { .. } => "tool_result",
        AgentEvent::Heartbeat { .. } => "heartbeat",
        AgentEvent::Reply { .. } => "reply",
    };
```

- [ ] **Step 3: Verify compilation**

```bash
cargo check --workspace
```

Expected: compiles successfully.

- [ ] **Step 4: Run existing tests**

```bash
cargo test --workspace --lib
```

Expected: all tests pass (serialization test for Reply will auto-work via serde tag).

- [ ] **Step 5: Commit**

```bash
git add crates/cctui-proto/src/ws.rs crates/cctui-server/src/ws.rs && git commit --no-gpg-sign -m "feat(proto): add Reply variant to AgentEvent (CCT-16)"
```

---

### Task 8: TUI — render Reply events distinctly

**Files:**
- Modify: `crates/cctui-tui/src/main.rs:430-461` (add Reply to agent_event_to_line)
- Modify: `crates/cctui-tui/src/app.rs:20-27` (add Reply to LineKind)
- Modify: `crates/cctui-tui/src/views/conversation.rs:224-293` (add Reply rendering)

- [ ] **Step 1: Add Reply to LineKind**

In `crates/cctui-tui/src/app.rs`, change:

```rust
pub enum LineKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    System,
}
```

to:

```rust
pub enum LineKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    System,
    Reply,
}
```

- [ ] **Step 2: Add Reply mapping in agent_event_to_line**

In `crates/cctui-tui/src/main.rs`, find the `agent_event_to_line` function. It should have matches for Text, ToolCall, ToolResult, and Heartbeat. Add the Reply arm. Add after the Heartbeat match:

```rust
        AgentEvent::Reply { content, ts } => {
            Some(ConversationLine { timestamp: ts, kind: LineKind::Reply, text: content })
        }
```

- [ ] **Step 3: Add Reply rendering in conversation view**

In `crates/cctui-tui/src/views/conversation.rs`, in the `render_line` function, add a match arm for `LineKind::Reply` before the closing of the match. Add after `LineKind::System`:

```rust
        LineKind::Reply => {
            let prefix = Span::styled("◁ Reply: ", Style::default().fg(theme::ACCENT).bold());
            let content = Span::styled(&line.text, Style::default().fg(theme::TEXT));
            vec![Line::from(vec![prefix, content])]
        }
```

- [ ] **Step 4: Verify compilation**

```bash
cargo check --workspace
```

Expected: compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add crates/cctui-tui/ && git commit --no-gpg-sign -m "feat(tui): render Reply events with distinct styling (CCT-16)"
```

---

### Task 9: Update setup flow (hooks + .mcp.json)

**Files:**
- Modify: `scripts/setup-claude.sh`
- Delete: `scripts/bootstrap.sh.tpl`
- Delete: `scripts/streamer.py`
- Modify: `scripts/setup.py.tpl` (rewrite for new flow)

- [ ] **Step 1: Rewrite setup.py.tpl for the channel flow**

Replace `scripts/setup.py.tpl` with:

```python
#!/usr/bin/env python3
"""Setup script generated by cctui-server. Configures Claude Code to use cctui-channel."""
import json
import os

SERVER_URL = "__SERVER_URL__"
TOKEN = "__TOKEN__"
HOOK_PORT = "__HOOK_PORT__"

# --- Find channel directory ---
# The channel lives in the cctui repo. For installed setups, it could be
# at a well-known path. For now, we require CCTUI_CHANNEL_DIR to be set,
# or default to the repo-relative path.
channel_dir = os.environ.get(
    "CCTUI_CHANNEL_DIR",
    os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "channel"),
)

if not os.path.isdir(channel_dir):
    print(f"[cctui] ERROR: channel dir not found at {channel_dir}")
    print("        Set CCTUI_CHANNEL_DIR to the absolute path of the channel/ directory")
    exit(1)

# --- Write .mcp.json for Claude Code ---
# User-level config at ~/.claude.json (not project-level)
claude_json_path = os.path.expanduser("~/.claude.json")

mcp_entry = {
    "command": "bun",
    "args": ["run", os.path.join(channel_dir, "src", "index.ts")],
    "env": {
        "CCTUI_URL": SERVER_URL,
        "CCTUI_AGENT_TOKEN": TOKEN,
        "CCTUI_HOOK_PORT": HOOK_PORT,
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

hooks = {
    "SessionStart": [{
        "hooks": [{
            "type": "command",
            "command": f"curl -sf -X POST http://localhost:{HOOK_PORT}/hooks/session-start -H 'Content-Type: application/json' -d @-",
        }],
    }],
    "PreToolUse": [{
        "hooks": [{
            "type": "http",
            "url": f"http://localhost:{HOOK_PORT}/hooks/pre-tool-use",
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

print(f"[cctui] done - Claude Code will use cctui-channel connecting to {SERVER_URL}")
```

- [ ] **Step 2: Delete old scripts**

```bash
rm scripts/bootstrap.sh.tpl scripts/streamer.py
```

- [ ] **Step 3: Update setup-claude.sh**

Replace `scripts/setup-claude.sh` with:

```bash
#!/bin/sh
# Configure Claude Code to use the cctui-channel MCP server.
# Run this on any machine where you want Claude to connect to cctui-server.
#
# Usage: ./scripts/setup-claude.sh [server_url] [token] [hook_port]
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"
HOOK_PORT="${3:-8701}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "==> Installing channel dependencies..."
cd "$REPO_DIR/channel" && bun install --silent

echo "==> Configuring Claude Code..."
CCTUI_CHANNEL_DIR="$REPO_DIR/channel" \
  python3 -c "
SERVER_URL = '$URL'
TOKEN = '$TOKEN'
HOOK_PORT = '$HOOK_PORT'
$(sed 's/^SERVER_URL = .*//' "$SCRIPT_DIR/setup.py.tpl" | sed 's/^TOKEN = .*//' | sed 's/^HOOK_PORT = .*//')
"

echo ""
echo "==> Done! Next time you start Claude Code, the cctui-channel will activate."
echo "    Server: $URL"
echo "    Hook port: $HOOK_PORT"
```

Actually, the setup script is getting fragile with sed. Let's simplify — just call Python directly with env vars:

Replace `scripts/setup-claude.sh` with:

```bash
#!/bin/sh
# Configure Claude Code to use the cctui-channel MCP server.
set -e

URL="${1:-http://localhost:8700}"
TOKEN="${2:-dev-agent}"
HOOK_PORT="${3:-8701}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_DIR="$(dirname "$SCRIPT_DIR")"

echo "==> Installing channel dependencies..."
(cd "$REPO_DIR/channel" && bun install --silent)

echo "==> Configuring Claude Code hooks and MCP server..."
CCTUI_CHANNEL_DIR="$REPO_DIR/channel" \
  __SERVER_URL__="$URL" \
  __TOKEN__="$TOKEN" \
  __HOOK_PORT__="$HOOK_PORT" \
  python3 "$SCRIPT_DIR/setup.py.tpl"

echo ""
echo "==> Done! Next time you start Claude Code, the cctui-channel will activate."
echo "    Server: $URL"
echo "    Hook port: $HOOK_PORT"
```

And update `setup.py.tpl` to read from env vars with fallback to template strings:

At the top of `setup.py.tpl`, change the constants to:

```python
SERVER_URL = os.environ.get("__SERVER_URL__", "__SERVER_URL__")
TOKEN = os.environ.get("__TOKEN__", "__TOKEN__")
HOOK_PORT = os.environ.get("__HOOK_PORT__", "__HOOK_PORT__")
```

- [ ] **Step 4: Update Makefile setup/claude target**

In `Makefile`, the existing `setup/claude` target at line 66-67 already calls `setup-claude.sh`. Update it to pass the hook port:

Change:

```makefile
setup/claude:  ## Configure local Claude Code to auto-register with the server
	./scripts/setup-claude.sh $(CCTUI_URL) dev-agent
```

to:

```makefile
setup/claude:  ## Configure local Claude Code to use cctui-channel
	./scripts/setup-claude.sh $(CCTUI_URL) dev-agent 8701
```

- [ ] **Step 5: Verify the setup script runs without errors**

```bash
make setup/claude
```

Expected: installs channel deps, writes `.claude.json` and `~/.claude/settings.json`.

- [ ] **Step 6: Commit**

```bash
git add scripts/ Makefile && git commit --no-gpg-sign -m "feat(setup): update setup flow for MCP channel, retire old scripts (CCT-16)"
```

---

### Task 10: Update server bootstrap routes for new setup

**Files:**
- Modify: `crates/cctui-server/src/main.rs:39-64` (remove streamer/bootstrap script routes)
- Modify or delete: `crates/cctui-server/src/routes/bootstrap.rs` (if it exists)

- [ ] **Step 1: Check if bootstrap routes exist**

Look at `crates/cctui-server/src/main.rs` lines 56-58 which have routes for:
- `GET /api/v1/scripts/streamer.py`
- `GET /api/v1/scripts/bootstrap.sh`

And there's likely a `GET /api/v1/setup` route too. These serve the old Python/shell scripts. They can be removed since setup now happens locally.

Remove these routes from the router in `main.rs`. Also remove the `GET /api/v1/setup` route if present.

- [ ] **Step 2: Remove bootstrap route handler module**

If `crates/cctui-server/src/routes/bootstrap.rs` exists, delete it and remove the `pub mod bootstrap;` line from the routes module.

- [ ] **Step 3: Verify compilation**

```bash
cargo check --workspace
```

Expected: compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add crates/cctui-server/ && git commit --no-gpg-sign -m "chore(server): remove old bootstrap/streamer script routes (CCT-16)"
```

---

### Task 11: Update STATUS.md and CLAUDE.md

**Files:**
- Modify: `STATUS.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update STATUS.md**

In the "What's Missing / Needs Work" section:
- Remove section "1. Agent Sidecar" — replaced by channel
- Remove section "3. Bidirectional Messaging" — implemented
- Update the "Agent Integration" section under "What We Have" to reflect the new channel architecture

Add to "What We Have":

```markdown
### Channel (`channel/` — Bun/TypeScript)
- MCP channel server spawned by Claude Code via `.mcp.json`
- Bidirectional messaging: TUI → Claude via channel notifications, Claude → TUI via reply tool
- HTTP hook server for SessionStart (session registration) and PreToolUse (policy proxy)
- Transcript JSONL tailer replaces Python streamer
- Connects to cctui-server via REST for event posting and pending message polling
```

- [ ] **Step 2: Update CLAUDE.md**

Update the Architecture section to include the channel:

```
cctui-server (Axum, PostgreSQL)  ←→  cctui-tui (Ratatui)
     ↑
cctui-channel (Bun/TS MCP server) ←→ Claude Code
```

Add to the Workspace section:

```
channel/           # MCP channel server: bidirectional Claude ↔ TUI messaging
  src/             # index.ts, mcp.ts, hooks.ts, bridge.ts, transcript.ts
```

Remove references to cctui-shim, bootstrap.sh.tpl, streamer.py.

Update "How It Works" to reflect the new flow:
1. `make setup/claude` configures `.mcp.json` + hooks
2. Claude starts → spawns cctui-channel as MCP subprocess
3. SessionStart hook → channel HTTP → registers with server, starts transcript tailing
4. PreToolUse hook → channel HTTP → proxied to server for policy check
5. Transcript events streamed to server → broadcast to TUI
6. TUI messages queued on server → polled by channel → pushed as MCP notifications
7. Claude replies via `cctui_reply` tool → posted to server → broadcast to TUI

- [ ] **Step 3: Commit**

```bash
git add STATUS.md CLAUDE.md && git commit --no-gpg-sign -m "docs: update STATUS.md and CLAUDE.md for channel architecture (CCT-16)"
```

---

### Task 12: End-to-end smoke test

- [ ] **Step 1: Start the server**

```bash
make run/server
```

- [ ] **Step 2: Run the channel in isolation (without Claude Code)**

In a separate terminal:

```bash
cd channel && CCTUI_URL=http://localhost:8700 CCTUI_AGENT_TOKEN=dev-agent CCTUI_HOOK_PORT=8701 bun run src/index.ts
```

Note: It will connect MCP over stdio which won't work outside Claude Code, but the HTTP server should start.

- [ ] **Step 3: Test SessionStart hook manually**

```bash
curl -sf -X POST http://localhost:8701/hooks/session-start \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"test-123","cwd":"/tmp","model":"opus","transcript_path":"/tmp/test-transcript.jsonl"}'
```

Expected: `{"status":"ok"}` and session appears in server.

- [ ] **Step 4: Verify session registered**

```bash
curl -sf http://localhost:8700/api/v1/sessions -H 'Authorization: Bearer dev-admin' | jq .
```

Expected: session with id `test-123` appears in the list.

- [ ] **Step 5: Test PreToolUse hook**

```bash
curl -sf -X POST http://localhost:8701/hooks/pre-tool-use \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"test-123","tool_name":"Bash","tool_input":{"command":"ls"}}'
```

Expected: `{"decision":"allow"}`.

- [ ] **Step 6: Run all tests**

```bash
cd channel && bun test
cargo test --workspace --lib
```

Expected: all tests pass.

- [ ] **Step 7: Run lints**

```bash
make lint
cd channel && bunx tsc --noEmit
```

Expected: no errors.
