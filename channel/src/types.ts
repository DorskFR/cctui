import { readFileSync } from "fs";
import { join } from "path";
import { homedir } from "os";

/** Configuration — prefers ~/.config/cctui/machine.json, falls back to env. */
export interface Config {
  /** cctui-server base URL, e.g. "http://localhost:8700" */
  serverUrl: string;
  /** Bearer token — machine key (preferred) or legacy agent token. */
  agentToken: string;
}

interface MachineIdentity {
  server_url: string;
  machine_key: string;
}

function loadMachineIdentity(): MachineIdentity | null {
  const base = process.env.XDG_CONFIG_HOME ?? join(homedir(), ".config");
  const path = join(base, "cctui", "machine.json");
  try {
    const raw = readFileSync(path, "utf8");
    const parsed = JSON.parse(raw) as MachineIdentity;
    if (parsed.server_url && parsed.machine_key) {
      return parsed;
    }
  } catch {
    // missing / malformed — fall through to env
  }
  return null;
}

export function loadConfig(): Config {
  const identity = loadMachineIdentity();
  if (identity) {
    // Env vars may still override (useful for local dev / CI).
    return {
      serverUrl: process.env.CCTUI_URL ?? identity.server_url,
      agentToken: process.env.CCTUI_AGENT_TOKEN ?? identity.machine_key,
    };
  }
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

/** Permission request from Claude Code via MCP notification. */
export interface PermissionRequest {
  request_id: string;
  tool_name: string;
  description: string;
  input_preview: string;
}
