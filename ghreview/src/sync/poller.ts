import type { DbHandle } from "../db/client.ts";
import type { Subscription } from "../db/subscriptions.ts";
import { listActiveSubscriptions } from "../db/subscriptions.ts";
import type { EventBus } from "../events/bus.ts";
import type { Account } from "../github/account.ts";
import {
  type SyncContext,
  type SyncOutcome,
  syncNotifications,
  syncPull,
  syncRepo,
} from "./handlers.ts";
import { drainPendingReads } from "./notificationPush.ts";
import { drainPendingViewed } from "./viewedPush.ts";

export interface PollerOptions {
  db: DbHandle;
  account: Account;
  bus: EventBus;
  intervalMs: number;
  syncViewedFromGithub?: boolean;
}

const HANDLERS: Record<string, (ctx: SyncContext, sub: Subscription) => Promise<SyncOutcome>> = {
  repo: syncRepo,
  pull_request: syncPull,
  notification: syncNotifications,
};

export class Poller {
  private opts: PollerOptions;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private running = false;
  lastRun: string | null = null;

  constructor(opts: PollerOptions) {
    this.opts = opts;
  }

  async runOnce(): Promise<void> {
    const { db, account, bus } = this.opts;
    const subs = await listActiveSubscriptions(db);
    const mine = subs.filter((s) => s.account === account.login);
    if (mine.length === 0) {
      this.lastRun = new Date().toISOString();
      return;
    }
    bus.publishSyncStatus(account.login, "syncing", this.lastRun);
    let errored = false;
    for (const sub of mine) {
      if (!account.budget.canSpend()) break;
      const handler = HANDLERS[sub.kind];
      if (!handler) continue;
      try {
        const res = await handler(
          { db, account, syncViewedFromGithub: this.opts.syncViewedFromGithub },
          sub,
        );
        account.budget.record(res.status, res.rate);
        if (res.secondaryLimit) account.budget.noteSecondaryLimit(res.retryAfter ?? undefined);
        if (res.status >= 400 && res.status !== 404) errored = true;
      } catch {
        errored = true;
      }
    }
    if (account.budget.canSpend()) {
      try {
        await drainPendingReads(db, account);
      } catch {
        errored = true;
      }
    }
    if (account.budget.canSpend()) {
      try {
        await drainPendingViewed(db, account);
      } catch {
        errored = true;
      }
    }
    this.lastRun = new Date().toISOString();
    bus.publishSyncStatus(account.login, errored ? "error" : "idle", this.lastRun);
  }

  start(): void {
    if (this.timer) return;
    const tick = async () => {
      try {
        await this.runOnce();
      } catch {}
      if (this.running) {
        const wait = Math.max(this.opts.intervalMs, this.opts.account.budget.msUntilAvailable());
        this.timer = setTimeout(tick, wait);
      }
    };
    this.running = true;
    this.timer = setTimeout(tick, 0);
  }

  stop(): void {
    this.running = false;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
