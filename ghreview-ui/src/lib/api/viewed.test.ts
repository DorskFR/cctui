import { describe, expect, it } from "vitest";
import type { ViewedStateResult } from "./types";
import { applyOptimisticViewed, viewedSet } from "./viewed";

function result(items: Array<[string, boolean]>): ViewedStateResult {
  return {
    items: items.map(([path, viewed]) => ({
      path,
      viewed,
      digest: null,
      push_pending: false,
      last_error: null,
      updated_at: null,
    })),
  };
}

describe("viewedSet", () => {
  it("collects only viewed paths", () => {
    const set = viewedSet(result([["a.ts", true], ["b.ts", false], ["c.ts", true]]));
    expect([...set].sort()).toEqual(["a.ts", "c.ts"]);
  });

  it("is empty for undefined input", () => {
    expect(viewedSet(undefined).size).toBe(0);
  });
});

describe("applyOptimisticViewed", () => {
  it("marks paths viewed, adding rows that did not exist", () => {
    const next = applyOptimisticViewed(result([["a.ts", false]]), ["a.ts", "b.ts"], true);
    const byPath = Object.fromEntries(next.items.map((i) => [i.path, i.viewed]));
    expect(byPath).toEqual({ "a.ts": true, "b.ts": true });
    expect(next.items.every((i) => i.push_pending)).toBe(true);
  });

  it("unmarks without dropping the row", () => {
    const next = applyOptimisticViewed(result([["a.ts", true]]), ["a.ts"], false);
    expect(next.items).toHaveLength(1);
    expect(next.items[0].viewed).toBe(false);
  });

  it("does not mutate the input (rollback safety)", () => {
    const prev = result([["a.ts", true]]);
    const snapshot = JSON.stringify(prev);
    applyOptimisticViewed(prev, ["a.ts"], false);
    expect(JSON.stringify(prev)).toBe(snapshot);
  });
});
