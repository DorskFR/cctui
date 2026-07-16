import { describe, expect, it } from "vitest";
import type { ViewedStateResult } from "./types";
import { applyOptimisticViewed, changedSinceViewed, viewedSet } from "./viewed";

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

function resultWithDigest(items: Array<[string, boolean, string | null]>): ViewedStateResult {
  return {
    items: items.map(([path, viewed, digest]) => ({
      path,
      viewed,
      digest,
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

describe("changedSinceViewed", () => {
  it("flags viewed files whose sha differs from the recorded digest", () => {
    const state = resultWithDigest([
      ["a.ts", true, "sha-old"],
      ["b.ts", true, "sha-b"],
    ]);
    const changed = changedSinceViewed(state, [
      { filename: "a.ts", sha: "sha-new" },
      { filename: "b.ts", sha: "sha-b" },
    ]);
    expect([...changed]).toEqual(["a.ts"]);
  });

  it("ignores unviewed files and files with no recorded digest or sha", () => {
    const state = resultWithDigest([
      ["a.ts", false, "sha-old"],
      ["b.ts", true, null],
      ["c.ts", true, "sha-old"],
    ]);
    const changed = changedSinceViewed(state, [
      { filename: "a.ts", sha: "sha-new" },
      { filename: "b.ts", sha: "sha-new" },
      { filename: "c.ts" },
    ]);
    expect(changed.size).toBe(0);
  });

  it("is empty for undefined state", () => {
    expect(changedSinceViewed(undefined, [{ filename: "a.ts", sha: "x" }]).size).toBe(0);
  });
});
