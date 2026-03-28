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
