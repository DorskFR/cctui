import type { DbHandle } from "../db/client.ts";
import { EVENT_CHANNEL } from "../db/documents.ts";

export interface SseMessage {
  event: string;
  data: unknown;
}

export interface DocumentNotice {
  account: string;
  kind: string;
  key: string;
}

export type SyncState = "idle" | "syncing" | "error";

export function mapNotice(notice: DocumentNotice): SseMessage | null {
  if (notice.kind === "pull_request") {
    const match = /^(.+?)\/(.+?)#(\d+)$/.exec(notice.key);
    if (!match) return null;
    return {
      event: "pr.updated",
      data: {
        account: notice.account,
        owner: match[1],
        repo: match[2],
        number: Number(match[3]),
      },
    };
  }
  if (notice.kind === "notification") {
    return { event: "notification.new", data: { account: notice.account, id: notice.key } };
  }
  return null;
}

type Listener = (msg: SseMessage) => void;

export class EventBus {
  private listeners = new Set<Listener>();
  private unlisten: (() => Promise<void>) | null = null;

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  emit(msg: SseMessage): void {
    for (const listener of this.listeners) listener(msg);
  }

  publishNotice(notice: DocumentNotice): void {
    const msg = mapNotice(notice);
    if (msg) this.emit(msg);
  }

  publishSyncStatus(account: string, state: SyncState, lastRun: string | null): void {
    this.emit({ event: "sync.status", data: { account, state, last_run: lastRun } });
  }

  async startListening(db: DbHandle): Promise<void> {
    const sub = await db.sql.listen(EVENT_CHANNEL, (payload) => {
      try {
        this.publishNotice(JSON.parse(payload) as DocumentNotice);
      } catch {}
    });
    this.unlisten = sub.unlisten;
  }

  async stop(): Promise<void> {
    if (this.unlisten) {
      await this.unlisten();
      this.unlisten = null;
    }
    this.listeners.clear();
  }
}
