import { describe, expect, test } from "bun:test";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";
import { fetchPullCommits } from "../src/sync/handlers.ts";

function commitEntry(i: number): Record<string, unknown> {
  return {
    sha: `sha${i}`.padEnd(40, "0"),
    commit: {
      message: `commit ${i}\n\nbody line`,
      author: { name: `Author ${i}`, date: "2026-07-13T00:00:00Z" },
    },
    author: { login: `author${i}` },
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

describe("fetchPullCommits", () => {
  test("returns commit entries from a single page", async () => {
    const { client } = mockOctokit(() => ({
      status: 200,
      headers: {},
      data: [commitEntry(0), commitEntry(1)],
    }));
    const commits = await fetchPullCommits(client, "DorskFR", "cctui", 42);
    expect(commits).toHaveLength(2);
    expect(commits[0]).toMatchObject({
      sha: expect.any(String),
      commit: { message: expect.any(String) },
      author: { login: "author0" },
    });
  });

  test("paginates until a short page is returned", async () => {
    const full = Array.from({ length: 100 }, (_, i) => commitEntry(i));
    const { client, calls } = mockOctokit((_route, params) =>
      params.page === 1
        ? { status: 200, headers: {}, data: full }
        : { status: 200, headers: {}, data: [commitEntry(100)] },
    );
    const commits = await fetchPullCommits(client, "DorskFR", "cctui", 42);
    expect(commits).toHaveLength(101);
    expect(calls.map((c) => c.page)).toEqual([1, 2]);
  });

  test("stops at an empty page without over-fetching", async () => {
    const { client, calls } = mockOctokit(() => ({ status: 200, headers: {}, data: [] }));
    const commits = await fetchPullCommits(client, "DorskFR", "cctui", 42);
    expect(commits).toHaveLength(0);
    expect(calls).toHaveLength(1);
  });
});
