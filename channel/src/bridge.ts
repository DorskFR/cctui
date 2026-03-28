import type { StreamerEvent, PreToolUsePayload, PendingMessage, ChannelRegisterResponse, SessionPollResponse } from "./types";

export interface PolicyVerdict {
  decision: "allow" | "deny";
  reason?: string;
}

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
    } catch {}
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
      return { decision: "allow" };
    }
  }

  async fetchPendingMessages(sessionId: string): Promise<PendingMessage[]> {
    try {
      const res = await fetch(`${this.baseUrl}/api/v1/sessions/${sessionId}/messages/pending`, { headers: this.headers() });
      if (!res.ok) return [];
      return res.json();
    } catch {
      return [];
    }
  }

  startPolling(sessionId: string, intervalMs = 1000): void {
    this.stopPolling();
    this.pollInterval = setInterval(async () => {
      const msgs = await this.fetchPendingMessages(sessionId);
      if (msgs.length > 0) {
        console.error(`[cctui-channel] polled ${msgs.length} pending message(s)`);
      }
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
}
