import { describe, expect, test } from "bun:test";
import {
  conditionalRequest,
  type OctokitRequest,
  type OctokitResponse,
} from "../src/github/client.ts";

function mockOctokit(
  handler: (route: string, params: Record<string, unknown>) => OctokitResponse | never,
): { client: OctokitRequest; calls: { route: string; params: Record<string, unknown> }[] } {
  const calls: { route: string; params: Record<string, unknown> }[] = [];
  const client: OctokitRequest = {
    request: async (route, params = {}) => {
      calls.push({ route, params });
      return handler(route, params);
    },
  };
  return { client, calls };
}

describe("conditionalRequest", () => {
  test("200 returns payload, etag and rate headers", async () => {
    const { client } = mockOctokit(() => ({
      status: 200,
      headers: { etag: 'W/"abc"', "x-ratelimit-remaining": "4990", "x-ratelimit-limit": "5000" },
      data: { full_name: "DorskFR/cctui" },
    }));
    const res = await conditionalRequest(client, "GET /repos/{owner}/{repo}", {
      owner: "DorskFR",
      repo: "cctui",
    });
    expect(res.status).toBe(200);
    expect(res.etag).toBe('W/"abc"');
    expect(res.rate.remaining).toBe(4990);
    expect(res.data).toEqual({ full_name: "DorskFR/cctui" });
  });

  test("sends If-None-Match when an etag is cached", async () => {
    const { client, calls } = mockOctokit(() => ({ status: 200, headers: {}, data: {} }));
    await conditionalRequest(client, "GET /x", {}, { etag: 'W/"cached"' });
    const sent = calls[0]?.params.headers as Record<string, string> | undefined;
    expect(sent?.["if-none-match"]).toBe('W/"cached"');
  });

  test("304 is surfaced as not-modified with the cached etag preserved", async () => {
    const { client } = mockOctokit(() => {
      throw { status: 304, response: { headers: { etag: 'W/"abc"' } } };
    });
    const res = await conditionalRequest(client, "GET /x", {}, { etag: 'W/"abc"' });
    expect(res.status).toBe(304);
    expect(res.data).toBeNull();
    expect(res.etag).toBe('W/"abc"');
  });

  test("secondary rate limit is flagged with retry-after", async () => {
    const { client } = mockOctokit(() => {
      throw {
        status: 403,
        response: { headers: { "retry-after": "60", "x-ratelimit-remaining": "5000" } },
      };
    });
    const res = await conditionalRequest(client, "GET /x", {});
    expect(res.status).toBe(403);
    expect(res.secondaryLimit).toBe(true);
    expect(res.retryAfter).toBe(60);
  });
});
