import { describe, expect, test, mock, beforeEach, afterEach } from "bun:test";
import { ServerBridge } from "../src/bridge";

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
      return new Response(JSON.stringify({ decision: "allow" }), { status: 200, headers: { "Content-Type": "application/json" } });
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const result = await bridge.checkPolicy({ session_id: "s1", tool_name: "Bash", tool_input: { command: "ls" } });
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

  test("registerSession throws on non-ok response", async () => {
    globalThis.fetch = mock(async () => new Response("bad", { status: 500 })) as typeof fetch;
    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    expect(bridge.registerSession({ claude_session_id: "x", machine_id: "m", working_dir: "/" })).rejects.toThrow("register failed");
  });

  test("postEvent swallows network errors", async () => {
    globalThis.fetch = mock(async () => { throw new Error("network down"); }) as typeof fetch;
    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    // Should not throw
    await bridge.postEvent("s1", { session_id: "s1", type: "text", content: "hi", ts: 0 });
  });

  test("checkPolicy returns allow on network error", async () => {
    globalThis.fetch = mock(async () => { throw new Error("timeout"); }) as typeof fetch;
    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const result = await bridge.checkPolicy({ session_id: "s1", tool_name: "Bash", tool_input: {} });
    expect(result.decision).toBe("allow");
  });

  test("fetchPendingMessages returns empty on error", async () => {
    globalThis.fetch = mock(async () => { throw new Error("fail"); }) as typeof fetch;
    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const msgs = await bridge.fetchPendingMessages("s1");
    expect(msgs).toEqual([]);
  });

  test("checkPolicy returns allow on non-ok status", async () => {
    globalThis.fetch = mock(async () => new Response("err", { status: 503 })) as typeof fetch;
    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const result = await bridge.checkPolicy({ session_id: "s1", tool_name: "Bash", tool_input: {} });
    expect(result.decision).toBe("allow");
  });

  test("fetchPendingMessages polls with provided session ID (must be server UUID)", async () => {
    let capturedUrl = "";
    globalThis.fetch = mock(async (input: string | URL | Request) => {
      capturedUrl = input.toString();
      return new Response(JSON.stringify([]), { status: 200, headers: { "Content-Type": "application/json" } });
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const serverUuid = "550e8400-e29b-41d4-a716-446655440000";
    await bridge.fetchPendingMessages(serverUuid);
    expect(capturedUrl).toBe(`http://localhost:8700/api/v1/sessions/${serverUuid}/messages/pending`);
  });

  test("startPolling calls onPendingMessage with fetched messages", async () => {
    const messages = [{ id: "msg-1", content: "hello from TUI", created_at: "2026-01-01T00:00:00Z" }];
    globalThis.fetch = mock(async () => {
      return new Response(JSON.stringify(messages), { status: 200, headers: { "Content-Type": "application/json" } });
    }) as typeof fetch;

    const bridge = new ServerBridge("http://localhost:8700", "dev-agent");
    const received: { id: string; content: string }[] = [];
    bridge.onPendingMessage = (msg) => received.push(msg);
    bridge.startPolling("server-uuid-123", 50); // 50ms interval for fast test

    // Wait for at least one poll cycle
    await new Promise((r) => setTimeout(r, 100));
    bridge.stopPolling();

    expect(received.length).toBeGreaterThanOrEqual(1);
    expect(received[0].content).toBe("hello from TUI");
  });

  test("registerSession sends auth header", async () => {
    let capturedHeaders: Record<string, string> = {};
    globalThis.fetch = mock(async (_url: string | URL | Request, init?: RequestInit) => {
      capturedHeaders = Object.fromEntries(Object.entries(init?.headers ?? {}));
      return new Response(JSON.stringify({ session_id: "x", ws_url: "ws://x" }));
    }) as typeof fetch;
    const bridge = new ServerBridge("http://localhost:8700", "my-token");
    await bridge.registerSession({ claude_session_id: "x", machine_id: "m", working_dir: "/" });
    expect(capturedHeaders["Authorization"]).toBe("Bearer my-token");
  });

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
});
