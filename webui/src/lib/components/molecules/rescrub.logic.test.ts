import { describe, expect, it } from "vitest";
import { rescrubCategoryRows, rescrubScopeSince } from "./rescrub.logic";
import type { RescrubReport } from "@bindings/RescrubReport";

const NOW = new Date("2026-09-08T12:00:00.000Z");

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
  const report = (by_category: Record<string, number>): RescrubReport => ({
    dry_run: true,
    rows_scanned: 10,
    rows_changed: 3,
    substitutions: 6,
    by_category,
  });

  it("has nothing to show before a scan", () => {
    expect(rescrubCategoryRows(null)).toEqual([]);
  });

  it("leads with whatever leaked most, breaking ties by name", () => {
    expect(rescrubCategoryRows(report({ jwt: 1, secret_field: 4, cctui_token: 1 }))).toEqual([
      { category: "secret_field", count: 4 },
      { category: "cctui_token", count: 1 },
      { category: "jwt", count: 1 },
    ]);
  });
});
