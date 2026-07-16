import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { runMigrations } from "../src/db/migrate.ts";
import type { AppDeps } from "../src/deps.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";
import { reduceReviewStates } from "../src/routes/reviewers.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA"));
const A = { authorization: "Bearer tokA", "content-type": "application/json" };

interface OctoOpts {
  reviews?: Record<string, unknown>[];
  requestedReviewers?: { login: string; avatar_url?: string }[];
  requestedTeams?: { name: string; slug: string }[];
  reRequestCalls?: Record<string, unknown>[];
}

function reviewersOctokit(opts: OctoOpts = {}): OctokitRequest {
  return {
    request: async (route: string, params?: Record<string, unknown>): Promise<OctokitResponse> => {
      if (route.includes("/reviews")) {
        const page = Number(params?.page ?? 1);
        return { status: 200, headers: {}, data: page === 1 ? (opts.reviews ?? []) : [] };
      }
      if (route.startsWith("POST") && route.endsWith("/requested_reviewers")) {
        opts.reRequestCalls?.push(params ?? {});
        return { status: 200, headers: {}, data: {} };
      }
      return {
        status: 200,
        headers: {},
        data: {
          number: 42,
          requested_reviewers: opts.requestedReviewers ?? [],
          requested_teams: opts.requestedTeams ?? [],
        },
      };
    },
  };
}

function deps(octokit: OctokitRequest): AppDeps {
  const account = createAccount({ login: "alpha", token: undefined, octokit });
  return { db, auth, accountFor: (login) => (login === "alpha" ? account : undefined) };
}

const URL = "/v1/repos/alpha/repo/pulls/42/reviewers";

describe("reduceReviewStates", () => {
  test("keeps the latest non-COMMENTED state per reviewer", () => {
    const states = reduceReviewStates([
      { user: "bob", avatar_url: "a", state: "COMMENTED" },
      { user: "bob", avatar_url: "a", state: "CHANGES_REQUESTED" },
      { user: "bob", avatar_url: "a", state: "APPROVED" },
      { user: "carol", avatar_url: "c", state: "APPROVED" },
      { user: "carol", avatar_url: "c", state: "COMMENTED" },
    ]);
    expect(states.get("bob")?.state).toBe("APPROVED");
    expect(states.get("carol")?.state).toBe("APPROVED");
  });

  test("falls back to COMMENTED when there is no verdict", () => {
    const states = reduceReviewStates([{ user: "dave", avatar_url: null, state: "COMMENTED" }]);
    expect(states.get("dave")?.state).toBe("COMMENTED");
  });
});

guarded("reviewers endpoints", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string, "ghreview");
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM gh_accounts");
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: "x" });
  });

  test("combines requested reviewers with reduced review states", async () => {
    const app = createApp(
      deps(
        reviewersOctokit({
          reviews: [
            { user: { login: "bob", avatar_url: "b" }, state: "APPROVED" },
            { user: { login: "carol", avatar_url: "c" }, state: "CHANGES_REQUESTED" },
          ],
          requestedReviewers: [{ login: "erin", avatar_url: "e" }],
          requestedTeams: [{ name: "Platform", slug: "platform" }],
        }),
      ),
    );
    const res = await app.request(`${URL}?account=alpha`, { headers: A });
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      reviewers: { login: string; state: string; requested: boolean }[];
      requested_teams: { slug: string }[];
    };
    const byLogin = Object.fromEntries(body.reviewers.map((r) => [r.login, r]));
    expect(byLogin.bob?.state).toBe("APPROVED");
    expect(byLogin.carol?.state).toBe("CHANGES_REQUESTED");
    expect(byLogin.erin?.state).toBe("PENDING");
    expect(byLogin.erin?.requested).toBe(true);
    expect(body.requested_teams[0]?.slug).toBe("platform");
  });

  test("surfaces a requested team when no individual reviewer or review exists", async () => {
    const app = createApp(
      deps(
        reviewersOctokit({
          reviews: [],
          requestedReviewers: [],
          requestedTeams: [{ name: "Grid-devs", slug: "grid-devs" }],
        }),
      ),
    );
    const res = await app.request(`${URL}?account=alpha`, { headers: A });
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      reviewers: { login: string }[];
      requested_teams: { name: string; slug: string }[];
    };
    expect(body.reviewers).toEqual([]);
    expect(body.requested_teams).toEqual([{ name: "Grid-devs", slug: "grid-devs" }]);
  });

  test("re-requests reviews from the given reviewers", async () => {
    const reRequestCalls: Record<string, unknown>[] = [];
    const app = createApp(deps(reviewersOctokit({ reRequestCalls })));
    const res = await app.request(`${URL}/re-request`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", reviewers: ["bob"] }),
    });
    expect(res.status).toBe(200);
    expect(reRequestCalls[0]?.reviewers).toEqual(["bob"]);
  });
});
