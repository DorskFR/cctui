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
    gitBranch = execSync(`git -C "${cwd}" rev-parse --abbrev-ref HEAD`, {
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

      // Poll using the same session ID — server now uses it as primary key
      bridge.startPolling(payload.session_id);

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
// HTTP server must start BEFORE MCP connect — the SessionStart hook fires
// as soon as Claude Code finishes spawning MCP servers, so the HTTP endpoint
// needs to be listening before the stdio handshake completes.
const hookServer = createHookServer({
  port: config.hookPort,
  onSessionStart,
  onPreToolUse,
});

console.error(`[cctui-channel] hook server listening on :${hookServer.port}`);

await connect();
console.error(`[cctui-channel] connected to Claude Code, waiting for SessionStart hook...`);

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
