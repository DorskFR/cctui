import { describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import type { AppDeps } from "../src/deps.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";

const auth = createStaticResolver(parseStaticTokens("tokA:userA"));
const A = { authorization: "Bearer tokA" };

function reposOctokit(pages: Record<string, unknown>[][]): OctokitRequest {
  return {
    request: async (_route, params = {}) => {
      const page = Number((params as { page?: number }).page ?? 1);
      const data = pages[page - 1] ?? [];
      const res: OctokitResponse = { status: 200, headers: {}, data };
      return res;
    },
  };
}

function appWith(octokit: OctokitRequest, extra: Partial<AppDeps> = {}) {
  const account = createAccount({ login: "alpha", token: undefined, octokit });
  return createApp({
    auth,
    accountFor: (login) => (login === "alpha" ? account : undefined),
    ...extra,
  });
}

describe("GET /v1/github/repos", () => {
  test("lists accessible repos GitHub-shaped, paginating until a short page", async () => {
    const full = Array.from({ length: 100 }, (_, i) => ({
      full_name: `alpha/repo${i}`,
      private: i % 2 === 0,
      permissions: { push: true, admin: false },
      pushed_at: "2026-07-12T09:00:00Z",
    }));
    const app = appWith(reposOctokit([full, [{ full_name: "alpha/last", private: false }]]));
    const res = await app.request("/v1/github/repos?account=alpha", { headers: A });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { items: { full_name: string; permissions: unknown }[] };
    expect(body.items.length).toBe(101);
    expect(body.items[0]?.full_name).toBe("alpha/repo0");
    expect(body.items[0]?.permissions).toEqual({ push: true, admin: false });
    expect(body.items[100]?.full_name).toBe("alpha/last");
  });

  test("404s when the account is not managed", async () => {
    const app = appWith(reposOctokit([[]]));
    const res = await app.request("/v1/github/repos?account=ghost", { headers: A });
    expect(res.status).toBe(404);
  });

  test("400s when account is missing", async () => {
    const app = appWith(reposOctokit([[]]));
    const res = await app.request("/v1/github/repos", { headers: A });
    expect(res.status).toBe(400);
  });
});
