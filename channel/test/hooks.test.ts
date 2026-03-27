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
      body: JSON.stringify({ session_id: "abc-123", tool_name: "Bash", tool_input: { command: "ls" } }),
    });

    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.decision).toBe("allow");
    expect(onPreToolUse).toHaveBeenCalledTimes(1);

    server.stop();
  });

  test("GET /health returns 200", async () => {
    const server = createHookServer({ port: 0, onSessionStart: () => {}, onPreToolUse: async () => ({ decision: "allow" }) });
    const res = await fetch(`http://localhost:${server.port}/health`);
    expect(res.status).toBe(200);
    server.stop();
  });

  test("unknown route returns 404", async () => {
    const server = createHookServer({ port: 0, onSessionStart: () => {}, onPreToolUse: async () => ({ decision: "allow" }) });
    const res = await fetch(`http://localhost:${server.port}/unknown`);
    expect(res.status).toBe(404);
    server.stop();
  });

  test("POST /hooks/session-start with malformed JSON returns 400", async () => {
    const server = createHookServer({ port: 0, onSessionStart: () => {}, onPreToolUse: async () => ({ decision: "allow" }) });
    const res = await fetch(`http://localhost:${server.port}/hooks/session-start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "not json",
    });
    expect(res.status).toBe(400);
    server.stop();
  });

  test("POST /hooks/pre-tool-use returns allow on callback error", async () => {
    const onPreToolUse = async () => { throw new Error("boom"); };
    const server = createHookServer({ port: 0, onSessionStart: () => {}, onPreToolUse });
    const res = await fetch(`http://localhost:${server.port}/hooks/pre-tool-use`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ session_id: "s1", tool_name: "Bash", tool_input: {} }),
    });
    expect(res.status).toBe(200);
    const body = await res.json();
    expect(body.decision).toBe("allow");
    server.stop();
  });

  test("POST /hooks/pre-tool-use with deny verdict", async () => {
    const onPreToolUse = async () => ({ decision: "deny" as const, reason: "blocked" });
    const server = createHookServer({ port: 0, onSessionStart: () => {}, onPreToolUse });
    const res = await fetch(`http://localhost:${server.port}/hooks/pre-tool-use`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ session_id: "s1", tool_name: "Bash", tool_input: {} }),
    });
    const body = await res.json();
    expect(body.decision).toBe("deny");
    expect(body.reason).toBe("blocked");
    server.stop();
  });
});
