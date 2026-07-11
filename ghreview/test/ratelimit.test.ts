import { describe, expect, test } from "bun:test";
import { BudgetTracker, parseRateHeaders } from "../src/github/ratelimit.ts";

describe("parseRateHeaders", () => {
  test("reads x-ratelimit-* case-insensitively", () => {
    const r = parseRateHeaders({
      "x-ratelimit-limit": "5000",
      "X-RateLimit-Remaining": "4990",
      "x-ratelimit-reset": "1800000000",
      "x-ratelimit-used": "10",
    });
    expect(r).toEqual({ limit: 5000, remaining: 4990, reset: 1800000000, used: 10 });
  });

  test("ignores missing/garbage values", () => {
    expect(parseRateHeaders({ "x-ratelimit-limit": "nope" }).limit).toBeUndefined();
  });
});

describe("BudgetTracker", () => {
  const opts = (now: () => number) => ({ limit: 5000, ceilingFraction: 0.2, now });

  test("ceiling is 20% of the limit", () => {
    const b = new BudgetTracker(opts(() => 0));
    expect(b.ceiling).toBe(1000);
  });

  test("304 responses do not spend budget", () => {
    const b = new BudgetTracker(opts(() => 0));
    for (let i = 0; i < 50; i++) b.record(304, { reset: 3600 });
    expect(b.spent).toBe(0);
    expect(b.canSpend()).toBe(true);
  });

  test("non-304 responses spend and trip the ceiling", () => {
    const b = new BudgetTracker(opts(() => 0));
    for (let i = 0; i < 999; i++) b.record(200, { reset: 3600 });
    expect(b.overBudget()).toBe(false);
    b.record(200, { reset: 3600 });
    expect(b.spent).toBe(1000);
    expect(b.overBudget()).toBe(true);
    expect(b.canSpend()).toBe(false);
  });

  test("spend counter resets when the rate window rolls over", () => {
    const now = 0;
    const b = new BudgetTracker(opts(() => now));
    for (let i = 0; i < 1000; i++) b.record(200, { reset: 100 });
    expect(b.overBudget()).toBe(true);
    b.record(200, { reset: 200 });
    expect(b.spent).toBe(1);
    expect(b.overBudget()).toBe(false);
  });

  test("secondary rate limit forces a backoff window", () => {
    let now = 0;
    const b = new BudgetTracker(opts(() => now));
    b.noteSecondaryLimit(30);
    expect(b.inBackoff()).toBe(true);
    expect(b.canSpend()).toBe(false);
    expect(b.msUntilAvailable()).toBe(30_000);
    now = 30_001;
    expect(b.inBackoff()).toBe(false);
    expect(b.canSpend()).toBe(true);
  });

  test("msUntilAvailable waits for reset when over budget", () => {
    const now = 0;
    const b = new BudgetTracker(opts(() => now));
    for (let i = 0; i < 1000; i++) b.record(200, { reset: 3600 });
    expect(b.msUntilAvailable()).toBe(3_600_000);
  });
});
