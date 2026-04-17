#!/usr/bin/env bun
import { loadConfig } from "./types";
import type { SessionState } from "./types";
import { createChannelServer } from "./mcp";
import { ServerBridge } from "./bridge";
import { tailTranscript } from "./transcript";
import { walkProjectDirs, uploadIfChanged, type ProjectFile } from "./archive";
import { syncSkills } from "./skills";
import { hostname, homedir } from "os";
import { basename, dirname, join as pathJoin } from "path";

const config = loadConfig();
const bridge = new ServerBridge(config.serverUrl, config.agentToken);

let session: SessionState | null = null;
let tailAbort: AbortController | null = null;
let archiveInterval: ReturnType<typeof setInterval> | null = null;
let currentTranscriptAbs: string | null = null;

function currentProjectFile(): ProjectFile | null {
  if (!currentTranscriptAbs) return null;
  return {
    absPath: currentTranscriptAbs,
    projectDir: basename(dirname(currentTranscriptAbs)),
    sessionId: basename(currentTranscriptAbs, ".jsonl"),
  };
}

// --- MCP channel server (stdio) ---
const { pushMessage, sendPermissionResponse, connect } = createChannelServer({
  onReply: async (text) => {
    if (!session) return;
    await bridge.postEvent(session.sessionId, {
      session_id: session.sessionId,
      type: "assistant_message",
      content: `[Reply to TUI] ${text}`,
      ts: Math.floor(Date.now() / 1000),
    });
  },
  onPermissionRequest: async (requestId, toolName, description, inputPreview) => {
    if (!session) {
      console.error("[cctui-channel] permission_request received but no session — allowing");
      await sendPermissionResponse(requestId, "allow");
      return;
    }

    console.error(
      `[cctui-channel] forwarding permission_request to server: ${requestId} (${toolName})`,
    );

    try {
      await bridge.submitPermissionRequest(session.sessionId, {
        request_id: requestId,
        tool_name: toolName,
        description,
        input_preview: inputPreview,
      });
    } catch (err) {
      console.error(`[cctui-channel] failed to submit permission request for ${requestId} — allowing:`, err);
      await sendPermissionResponse(requestId, "allow");
      return;
    }

    const behavior = await bridge.pollPermissionDecision(session.sessionId, requestId);
    if (behavior === "allow" || behavior === "deny") {
      await sendPermissionResponse(requestId, behavior);
    } else {
      console.error(
        `[cctui-channel] permission decision timed out for ${requestId} — allowing`,
      );
      await sendPermissionResponse(requestId, "allow");
    }
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

        // Start tailing transcript — forward raw lines to server for lossless storage and parsing
        if (session.transcriptPath) {
          tailAbort = new AbortController();
          tailTranscript(
            session.transcriptPath,
            (line) => bridge.postTranscriptLine(poll.session_id, line),
            tailAbort.signal,
          );
        }

        // --- Archive pipeline ---
        currentTranscriptAbs = session.transcriptPath;
        const projectsRoot =
          process.env.CLAUDE_PROJECTS_DIR ?? pathJoin(homedir(), ".claude", "projects");

        // Startup scan: fire-and-forget, skip current session (periodic covers it).
        (async () => {
          const files = walkProjectDirs(projectsRoot);
          for (const f of files) {
            if (currentTranscriptAbs && f.absPath === currentTranscriptAbs) continue;
            await uploadIfChanged(bridge, f);
          }
        })().catch((err) =>
          console.error("[cctui-channel] startup archive scan failed:", err),
        );

        // Periodic flush for live session.
        const intervalMin = Number(process.env.CCTUI_ARCHIVE_INTERVAL_MINUTES ?? 15);
        const intervalMs = Math.max(1, intervalMin) * 60_000;
        archiveInterval = setInterval(async () => {
          const f = currentProjectFile();
          if (!f) return;
          await uploadIfChanged(bridge, f).catch((err) =>
            console.error("[cctui-channel] periodic archive failed:", err),
          );
        }, intervalMs);

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

// Skills sync is independent of session match — kick it off on startup.
syncSkills(bridge).catch((err) =>
  console.error("[cctui-channel] skill sync failed:", err),
);

async function finalFlush(): Promise<void> {
  tailAbort?.abort();
  bridge.stopPolling();
  if (archiveInterval) {
    clearInterval(archiveInterval);
    archiveInterval = null;
  }
  const f = currentProjectFile();
  if (f) {
    await uploadIfChanged(bridge, f).catch((err) =>
      console.error("[cctui-channel] final archive flush failed:", err),
    );
  }
}

process.on("SIGTERM", async () => {
  await finalFlush();
  process.exit(0);
});

process.on("SIGINT", async () => {
  await finalFlush();
  process.exit(0);
});
