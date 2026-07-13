import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { runMigrations } from "../src/db/migrate.ts";
import type { AppDeps } from "../src/deps.ts";
import { EventBus, type SseMessage } from "../src/events/bus.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));

const A = { authorization: "Bearer tokA", "content-type": "application/json" };
const B = { authorization: "Bearer tokB", "content-type": "application/json" };

interface OctoState {
  deletes: string[];
  reviewNumber: number;
  issueNumber: number;
  failStatus?: number;
}

function commentsOctokit(state: OctoState): OctokitRequest {
  return {
    request: async (route: string): Promise<OctokitResponse> => {
      if (route.startsWith("GET") && route.includes("/pulls/comments/")) {
        return {
          status: 200,
          headers: {},
          data: {
            pull_request_url: `https://api.github.com/repos/o/r/pulls/${state.reviewNumber}`,
          },
        };
      }
      if (route.startsWith("GET") && route.includes("/issues/comments/")) {
        return {
          status: 200,
          headers: {},
          data: { issue_url: `https://api.github.com/repos/o/r/issues/${state.issueNumber}` },
        };
      }
      if (route.startsWith("DELETE") && route.includes("/comments/")) {
        if (state.failStatus) {
          throw Object.assign(new Error("github says no"), {
            status: state.failStatus,
            response: { data: { message: "You are not allowed to delete this comment" } },
          });
        }
        state.deletes.push(route);
        return { status: 204, headers: {}, data: null };
      }
      return { status: 200, headers: {}, data: {} };
    },
  };
}

function deps(octokit: OctokitRequest, bus?: EventBus): AppDeps {
  const account = createAccount({ login: "alpha", token: undefined, octokit });
  return { db, bus, auth, accountFor: (login) => (login === "alpha" ? account : undefined) };
}

guarded("delete published comments", () => {
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
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: "y" });
  });

  test("deletes a published review comment and fires a pr.updated notice", async () => {
    const state: OctoState = { deletes: [], reviewNumber: 42, issueNumber: 7 };
    const bus = new EventBus();
    const events: SseMessage[] = [];
    bus.subscribe((m) => events.push(m));
    const app = createApp(deps(commentsOctokit(state), bus));
    const res = await app.request("/v1/repos/o/r/pulls/comments/9?account=alpha", {
      method: "DELETE",
      headers: A,
    });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ deleted: true });
    expect(state.deletes).toEqual(["DELETE /repos/o/r/pulls/comments/9"]);
    expect(events).toEqual([
      { event: "pr.updated", data: { account: "alpha", owner: "o", repo: "r", number: 42 } },
    ]);
  });

  test("deletes a published issue comment", async () => {
    const state: OctoState = { deletes: [], reviewNumber: 42, issueNumber: 7 };
    const app = createApp(deps(commentsOctokit(state)));
    const res = await app.request("/v1/repos/o/r/issues/comments/5?account=alpha", {
      method: "DELETE",
      headers: A,
    });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ deleted: true });
    expect(state.deletes).toEqual(["DELETE /repos/o/r/issues/comments/5"]);
  });

  test("maps a GitHub 403 to a clean 403 error body, not a 500", async () => {
    const state: OctoState = { deletes: [], reviewNumber: 42, issueNumber: 7, failStatus: 403 };
    const app = createApp(deps(commentsOctokit(state)));
    const res = await app.request("/v1/repos/o/r/pulls/comments/9?account=alpha", {
      method: "DELETE",
      headers: A,
    });
    expect(res.status).toBe(403);
    const body = (await res.json()) as { error: { code: string; message: string } };
    expect(body.error.code).toBe("forbidden");
    expect(body.error.message).toBe("You are not allowed to delete this comment");
  });

  test("maps a GitHub 404 to a clean 404 error body", async () => {
    const state: OctoState = { deletes: [], reviewNumber: 42, issueNumber: 7, failStatus: 404 };
    const app = createApp(deps(commentsOctokit(state)));
    const res = await app.request("/v1/repos/o/r/issues/comments/5?account=alpha", {
      method: "DELETE",
      headers: A,
    });
    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("not_found");
  });

  test("rejects a delete under an account the caller does not own", async () => {
    const state: OctoState = { deletes: [], reviewNumber: 42, issueNumber: 7 };
    const app = createApp(deps(commentsOctokit(state)));
    const res = await app.request("/v1/repos/o/r/pulls/comments/9?account=alpha", {
      method: "DELETE",
      headers: B,
    });
    expect(res.status).toBe(403);
    expect(state.deletes).toEqual([]);
  });
});
