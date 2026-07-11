import type { DbHandle } from "./db/client.ts";
import type { EventBus } from "./events/bus.ts";

export interface SyncSnapshot {
  last_run: string | null;
  accounts: string[];
}

export interface AppDeps {
  db?: DbHandle;
  bus?: EventBus;
  webhookSecret?: string;
  syncSnapshot?: () => SyncSnapshot;
}
