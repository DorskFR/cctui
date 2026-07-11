export interface RateHeaders {
  limit?: number;
  remaining?: number;
  reset?: number;
  used?: number;
}

export function parseRateHeaders(headers: Record<string, string | undefined>): RateHeaders {
  const lower: Record<string, string | undefined> = {};
  for (const [k, v] of Object.entries(headers)) lower[k.toLowerCase()] = v;
  const get = (name: string) => {
    const v = lower[name];
    if (v === undefined) return undefined;
    const n = Number(v);
    return Number.isFinite(n) ? n : undefined;
  };
  return {
    limit: get("x-ratelimit-limit"),
    remaining: get("x-ratelimit-remaining"),
    reset: get("x-ratelimit-reset"),
    used: get("x-ratelimit-used"),
  };
}

export interface BudgetOptions {
  limit: number;
  ceilingFraction: number;
  now?: () => number;
}

export class BudgetTracker {
  limit: number;
  ceilingFraction: number;
  remaining: number;
  resetAt: number;
  spent = 0;
  backoffUntil = 0;
  private windowKey = 0;
  private now: () => number;

  constructor(opts: BudgetOptions) {
    this.limit = opts.limit;
    this.ceilingFraction = opts.ceilingFraction;
    this.remaining = opts.limit;
    this.now = opts.now ?? Date.now;
    this.resetAt = this.now() + 3_600_000;
    this.windowKey = this.resetAt;
  }

  get ceiling(): number {
    return Math.floor(this.limit * this.ceilingFraction);
  }

  private rollWindow(resetMs: number): void {
    if (resetMs !== this.windowKey) {
      this.windowKey = resetMs;
      this.spent = 0;
    }
  }

  record(status: number, headers: RateHeaders): void {
    if (headers.limit !== undefined) this.limit = headers.limit;
    if (headers.reset !== undefined) {
      const resetMs = headers.reset * 1000;
      this.rollWindow(resetMs);
      this.resetAt = resetMs;
    }
    if (headers.remaining !== undefined) this.remaining = headers.remaining;
    // 304 responses do not count against the GitHub rate limit.
    if (status !== 304) this.spent += 1;
  }

  noteSecondaryLimit(retryAfterSeconds: number | undefined): void {
    const wait = (retryAfterSeconds ?? 60) * 1000;
    this.backoffUntil = Math.max(this.backoffUntil, this.now() + wait);
  }

  overBudget(): boolean {
    if (this.now() >= this.resetAt) return false;
    return this.spent >= this.ceiling;
  }

  inBackoff(): boolean {
    return this.now() < this.backoffUntil;
  }

  canSpend(): boolean {
    return !this.inBackoff() && !this.overBudget();
  }

  msUntilAvailable(): number {
    const t = this.now();
    if (this.inBackoff()) return this.backoffUntil - t;
    if (this.overBudget()) return Math.max(0, this.resetAt - t);
    return 0;
  }
}
