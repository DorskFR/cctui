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
  console.error(`[cctui-channel] received pending message: ${msg.content.slice(0, 100)}`);
  pushMessage(msg.content, { message_id: msg.id });
};

// --- Registration with retry ---
async function registerWithRetry(sessionId: string, machineId: string, cwd: string, gitBranch: string) {
  const maxRetries = 30;
  for (let attempt = 1; attempt <= maxRetries; attempt++) {
    try {
      await bridge.registerSession({
        claude_session_id: sessionId,
        machine_id: machineId,
        working_dir: cwd,
        metadata: {
          git_branch: gitBranch,
          project_name: basename(cwd),
          model: session?.model ?? "",
          transcript_path: session?.transcriptPath ?? "",
        },
      });
      console.error(`[cctui-channel] session registered: ${sessionId}`);

      bridge.startPolling(sessionId);

      if (session?.transcriptPath) {
        tailAbort = new AbortController();
        tailTranscript(
          sessionId,
          session.transcriptPath,
          (event) => bridge.postEvent(sessionId, event),
          tailAbort.signal,
        );
      }
      return;
    } catch (err) {
      console.error(`[cctui-channel] registration attempt ${attempt}/${maxRetries} failed:`, err);
      if (attempt < maxRetries) {
        await Bun.sleep(2000);
      }
    }
  }
  console.error("[cctui-channel] registration failed after all retries");
}

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

  registerWithRetry(payload.session_id, machineId, cwd, gitBranch);
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
