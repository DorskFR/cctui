import { type GhAccountWithSecret, listAllActiveAccounts } from "../db/accounts.ts";
import type { DbHandle } from "../db/client.ts";
import type { EventBus } from "../events/bus.ts";
import { type Account, createAccount } from "../github/account.ts";
import { Poller } from "./poller.ts";

export interface ManagerDefaults {
  pollIntervalMs: number;
  budgetCeilingFraction: number;
  rateLimitPerHour: number;
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
}

export class AccountManager {
  private opts: ManagerOptions;
  private managed = new Map<string, Managed>();
  private timer: ReturnType<typeof setInterval> | null = null;

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
      if (this.managed.has(row.login)) continue;
      let token: string;
      try {
        token = this.opts.open(row.encrypted_pat);
      } catch {
        continue;
      }
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
      });
      poller.start();
      this.managed.set(row.login, { account, poller });
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
