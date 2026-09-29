import { describe, expect, it } from "vitest";
import {
  rescrubCategoryRows,
  rescrubIdentifierWarnings,
  rescrubIsRunning,
  rescrubProgress,
  rescrubSampleLines,
  rescrubScopeSince,
} from "./rescrub.logic";
import type { PrivacyScanJob } from "@bindings/PrivacyScanJob";
import type { ScanCategory } from "@bindings/ScanCategory";

const NOW = new Date("2026-09-08T12:00:00.000Z");

const category = (over: Partial<ScanCategory> = {}): ScanCategory => ({
  category: "github_token",
  count: 1,
  samples: [],
  identifier_warning: false,
  ...over,
});

const job = (over: Partial<PrivacyScanJob> = {}): PrivacyScanJob => ({
  id: "job-1",
  status: "running",
  dry_run: true,
  cancel_requested: false,
  rows_total: 100,
  rows_scanned: 20,
  rows_changed: 3,
  substitutions: 6,
  by_category: {},
  categories: [],
  error: null,
  created_at: NOW.toISOString(),
  finished_at: null,
  ...over,
});

describe("rescrubScopeSince", () => {
  it("leaves all history unbounded", () => {
    expect(rescrubScopeSince("all", NOW)).toBeNull();
  });

  it("maps the windowed scopes to an RFC3339 instant", () => {
    expect(rescrubScopeSince("30d", NOW)).toBe("2026-08-09T12:00:00.000Z");
    expect(rescrubScopeSince("7d", NOW)).toBe("2026-09-01T12:00:00.000Z");
  });
});

describe("rescrubCategoryRows", () => {
  it("has nothing to show before a scan", () => {
    expect(rescrubCategoryRows(null)).toEqual([]);
  });

  it("keeps the server's ordering", () => {
    const categories = [category({ category: "secret_field", count: 4 }), category()];
    expect(rescrubCategoryRows(job({ categories }))).toEqual(categories);
  });
});

describe("rescrubProgress", () => {
  it("is determinate once the estimate is known", () => {
    expect(rescrubProgress(job())).toEqual({ value: 20, max: 100 });
  });

  it("falls back to indeterminate with no estimate or an overrun", () => {
    expect(rescrubProgress(null)).toBeNull();
    expect(rescrubProgress(job({ rows_total: null }))).toBeNull();
    expect(rescrubProgress(job({ rows_total: 0 }))).toBeNull();
    expect(rescrubProgress(job({ rows_total: 10, rows_scanned: 11 }))).toBeNull();
  });
});

describe("rescrubIsRunning", () => {
  it("is true only while the job runs", () => {
    expect(rescrubIsRunning(null)).toBe(false);
    expect(rescrubIsRunning(job())).toBe(true);
    expect(rescrubIsRunning(job({ status: "completed" }))).toBe(false);
    expect(rescrubIsRunning(job({ status: "cancelled" }))).toBe(false);
  });
});

describe("rescrubIdentifierWarnings", () => {
  it("names the categories the server flagged", () => {
    const categories = [
      category({ category: "mytoken", identifier_warning: true }),
      category(),
    ];
    expect(rescrubIdentifierWarnings(job({ categories }))).toEqual(["mytoken"]);
    expect(rescrubIdentifierWarnings(null)).toEqual([]);
  });
});

describe("rescrubSampleLines", () => {
  it("pairs each match with its context", () => {
    const c = category({
      samples: [{ text: "input_token", context: "the …count was 3", value_follows: false }],
    });
    expect(rescrubSampleLines(c)).toEqual(["input_token   the …count was 3"]);
    expect(rescrubSampleLines(category())).toEqual([]);
  });
});
