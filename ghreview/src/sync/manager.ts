import { type GhAccountWithSecret, listAllActiveAccounts } from "../db/accounts.ts";
import type { DbHandle } from "../db/client.ts";
import { upsertSubscription } from "../db/subscriptions.ts";
import type { EventBus } from "../events/bus.ts";
import { type Account, createAccount } from "../github/account.ts";
import { Poller } from "./poller.ts";

export type ForceSyncResult = "ok" | "busy" | "unknown";

export interface ManagerDefaults {
  pollIntervalMs: number;
  budgetCeilingFraction: number;
  rateLimitPerHour: number;
  syncViewedFromGithub?: boolean;
}

export interface ManagerOptions {
  db: DbHandle;
  bus: EventBus;
  defaults: ManagerDefaults;
  open: (sealed: string) => string;
  reloadMs?: number;
}

interface Managed {
  account: Account;
  poller: Poller;
  signature: string;
}

function signatureOf(row: GhAccountWithSecret): string {
  return [row.poll_interval_ms, row.budget_ceiling, row.rate_limit, row.encrypted_pat].join("|");
}

export class AccountManager {
  private opts: ManagerOptions;
  private managed = new Map<string, Managed>();
  private timer: ReturnType<typeof setInterval> | null = null;
  private forcing = new Set<string>();

  constructor(opts: ManagerOptions) {
    this.opts = opts;
  }

  async reload(): Promise<void> {
    let rows: GhAccountWithSecret[];
    try {
      rows = await listAllActiveAccounts(this.opts.db);
    } catch {
      return;
    }
    const seen = new Set<string>();
    for (const row of rows) {
      seen.add(row.login);
      const signature = signatureOf(row);
      const current = this.managed.get(row.login);
      if (current && current.signature === signature) continue;
      let token: string;
      try {
        token = this.opts.open(row.encrypted_pat);
      } catch {
        if (current) {
          current.poller.stop();
          this.managed.delete(row.login);
        }
        continue;
      }
      if (current) current.poller.stop();
      const account = createAccount({
        login: row.login,
        token,
        budget: {
          limit: row.rate_limit ?? this.opts.defaults.rateLimitPerHour,
          ceilingFraction: row.budget_ceiling ?? this.opts.defaults.budgetCeilingFraction,
        },
      });
      const poller = new Poller({
        db: this.opts.db,
        account,
        bus: this.opts.bus,
        intervalMs: row.poll_interval_ms ?? this.opts.defaults.pollIntervalMs,
        syncViewedFromGithub: this.opts.defaults.syncViewedFromGithub,
      });
      poller.start();
      this.managed.set(row.login, { account, poller, signature });
      await upsertSubscription(this.opts.db, row.login, "notification", null, "notification").catch(
        () => {},
      );
    }
    for (const [login, m] of this.managed) {
      if (!seen.has(login)) {
        m.poller.stop();
        this.managed.delete(login);
      }
    }
  }

  accountFor(login: string): Account | undefined {
    return this.managed.get(login)?.account;
  }

  async forceSync(login: string): Promise<ForceSyncResult> {
    const m = this.managed.get(login);
    if (!m) return "unknown";
    if (this.forcing.has(login)) return "busy";
    this.forcing.add(login);
    try {
      await m.poller.runOnce();
      return "ok";
    } finally {
      this.forcing.delete(login);
    }
  }

  snapshot(): { last_run: string | null; accounts: string[] } {
    let last: string | null = null;
    for (const m of this.managed.values()) {
      if (m.poller.lastRun && (!last || m.poller.lastRun > last)) last = m.poller.lastRun;
    }
    return { last_run: last, accounts: [...this.managed.keys()].sort() };
  }

  async start(): Promise<void> {
    await this.reload();
    this.timer = setInterval(() => {
      void this.reload();
    }, this.opts.reloadMs ?? 60_000);
  }

  stop(): void {
    if (this.timer) {
      clearInterval(this.timer);
      this.timer = null;
    }
    for (const m of this.managed.values()) m.poller.stop();
    this.managed.clear();
  }
}
