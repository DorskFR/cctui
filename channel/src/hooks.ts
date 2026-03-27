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
          return Response.json({ error: String(err) }, { status: 400 });
        }
      }

      if (url.pathname === "/hooks/pre-tool-use" && req.method === "POST") {
        try {
          const payload: PreToolUsePayload = await req.json();
          const verdict = await onPreToolUse(payload);
          return Response.json(verdict);
        } catch {
          return Response.json({ decision: "allow" });
        }
      }

      return new Response("not found", { status: 404 });
    },
  });

  return server;
}
