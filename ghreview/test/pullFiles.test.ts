import { describe, expect, test } from "bun:test";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";
import { enrichPullStats, fetchPullFiles, pullStatsFromFiles } from "../src/sync/pullEnrich.ts";

function fileEntry(i: number): Record<string, unknown> {
  return {
    filename: `src/file${i}.ts`,
    status: "modified",
    additions: 1,
    deletions: 0,
    changes: 1,
    patch: `@@ -1 +1 @@\n-old${i}\n+new${i}`,
  };
}

function mockOctokit(
  handler: (route: string, params: Record<string, unknown>) => OctokitResponse,
): { client: OctokitRequest; calls: Record<string, unknown>[] } {
  const calls: Record<string, unknown>[] = [];
  const client: OctokitRequest = {
    request: async (route, params = {}) => {
      calls.push(params);
      return handler(route, params);
    },
  };
  return { client, calls };
}

describe("fetchPullFiles", () => {
  test("returns the file entries with patch shape from a single page", async () => {
    const { client } = mockOctokit(() => ({
      status: 200,
      headers: {},
      data: [fileEntry(0), fileEntry(1)],
    }));
    const files = await fetchPullFiles(client, "DorskFR", "cctui", 42);
    expect(files).toHaveLength(2);
    expect(files[0]).toMatchObject({ filename: "src/file0.ts", patch: expect.any(String) });
  });

  test("paginates until a short page is returned", async () => {
    const full = Array.from({ length: 100 }, (_, i) => fileEntry(i));
    const { client, calls } = mockOctokit((_route, params) =>
      params.page === 1
        ? { status: 200, headers: {}, data: full }
        : { status: 200, headers: {}, data: [fileEntry(100)] },
    );
    const files = await fetchPullFiles(client, "DorskFR", "cctui", 42);
    expect(files).toHaveLength(101);
    expect(calls.map((c) => c.page)).toEqual([1, 2]);
  });

  test("stops at an empty page without over-fetching", async () => {
    const { client, calls } = mockOctokit(() => ({ status: 200, headers: {}, data: [] }));
    const files = await fetchPullFiles(client, "DorskFR", "cctui", 42);
    expect(files).toHaveLength(0);
    expect(calls).toHaveLength(1);
  });
});

describe("pullStatsFromFiles", () => {
  test("sums additions/deletions and counts changed files", () => {
    const files = [
      { filename: "a.ts", additions: 3, deletions: 1 },
      { filename: "b.ts", additions: 5, deletions: 0 },
    ];
    expect(pullStatsFromFiles(files)).toEqual({ additions: 8, deletions: 1, changed_files: 2 });
  });

  test("ignores non-numeric stat fields", () => {
    const files = [{ filename: "a.ts" }, { filename: "b.ts", additions: 2, deletions: 4 }];
    expect(pullStatsFromFiles(files)).toEqual({ additions: 2, deletions: 4, changed_files: 2 });
  });
});

describe("enrichPullStats", () => {
  test("keeps stats the payload already carries (single-PR GET)", () => {
    const files = [{ filename: "a.ts", additions: 3, deletions: 1 }];
    const out = enrichPullStats(
      { number: 1, additions: 40, deletions: 20, changed_files: 7 },
      files,
    );
    expect(out).toMatchObject({ additions: 40, deletions: 20, changed_files: 7 });
  });

  test("backfills stats from files when the payload omits them (list payload)", () => {
    const files = [
      { filename: "a.ts", additions: 3, deletions: 1 },
      { filename: "b.ts", additions: 5, deletions: 2 },
    ];
    const out = enrichPullStats({ number: 1 }, files);
    expect(out).toMatchObject({ additions: 8, deletions: 3, changed_files: 2 });
  });
});
