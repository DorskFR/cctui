import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { runMigrations } from "../src/db/migrate.ts";
import type { AppDeps } from "../src/deps.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";
import { normalizeTimelineEvent } from "../src/routes/activity.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA"));
const A = { authorization: "Bearer tokA", "content-type": "application/json" };

function timelineOctokit(pages: Record<string, unknown>[][]): OctokitRequest {
  return {
    request: async (route: string, params?: Record<string, unknown>): Promise<OctokitResponse> => {
      if (route.includes("/timeline")) {
        const page = Number(params?.page ?? 1);
        const data = pages[page - 1] ?? [];
        const hasNext = page < pages.length;
        return {
          status: 200,
          headers: hasNext
            ? { link: `<https://api.github.com/x?page=${page + 1}>; rel="next"` }
            : {},
          data,
        };
      }
      return { status: 200, headers: {}, data: {} };
    },
  };
}

function deps(octokit: OctokitRequest): AppDeps {
  const account = createAccount({ login: "alpha", token: undefined, octokit });
  return { db, auth, accountFor: (login) => (login === "alpha" ? account : undefined) };
}

const URL = "/v1/repos/alpha/repo/pulls/42/activity";

describe("normalizeTimelineEvent", () => {
  test("drops unrenderable events", () => {
    expect(normalizeTimelineEvent({ event: "mentioned" })).toBeNull();
    expect(normalizeTimelineEvent({ event: "subscribed" })).toBeNull();
    expect(normalizeTimelineEvent({ event: "" })).toBeNull();
  });

  test("committed: sha/message/author with null actor and author date", () => {
    const ev = normalizeTimelineEvent({
      event: "committed",
      sha: "abcdef1234567890",
      message: "feat: do a thing\n\nbody line",
      author: { name: "Ada", date: "2026-07-01T00:00:00Z" },
    });
    expect(ev?.actor).toBeNull();
    expect(ev?.created_at).toBe("2026-07-01T00:00:00Z");
    expect(ev?.detail?.sha).toBe("abcdef1");
    expect(ev?.detail?.message).toBe("feat: do a thing");
    expect(ev?.detail?.author_name).toBe("Ada");
  });

  test("reviewed: preserves identity, full body, reactions, and reads user + submitted_at", () => {
    const fullBody = `please fix\n\n${"detail ".repeat(80)}`;
    const ev = normalizeTimelineEvent({
      id: 91,
      event: "reviewed",
      state: "changes_requested",
      body: fullBody,
      user: { login: "bob", avatar_url: "b" },
      submitted_at: "2026-07-02T00:00:00Z",
      html_url: "https://github.com/o/r/pull/42#pullrequestreview-91",
      reactions: { heart: 2, total_count: 2 },
    });
    expect(ev?.id).toBe("91");
    expect(ev?.actor).toEqual({ login: "bob", avatar_url: "b" });
    expect(ev?.created_at).toBe("2026-07-02T00:00:00Z");
    expect(ev?.detail?.state).toBe("CHANGES_REQUESTED");
    expect(ev?.detail?.body).toBe(fullBody.trim());
    expect(ev?.html_url).toContain("pullrequestreview-91");
    expect(ev?.reactions).toEqual({ heart: 2, total_count: 2 });
  });

  test("labeled: carries label name + color", () => {
    const ev = normalizeTimelineEvent({
      event: "labeled",
      actor: { login: "carol", avatar_url: "c" },
      created_at: "2026-07-03T00:00:00Z",
      label: { name: "bug", color: "d73a4a" },
    });
    expect(ev?.detail?.label).toEqual({ name: "bug", color: "d73a4a" });
  });

  test("review_requested: carries requested reviewer", () => {
    const ev = normalizeTimelineEvent({
      event: "review_requested",
      actor: { login: "carol" },
      created_at: "2026-07-03T00:00:00Z",
      requested_reviewer: { login: "erin", avatar_url: "e" },
    });
    expect(ev?.detail?.reviewer).toEqual({ login: "erin", avatar_url: "e" });
  });

  test("renamed: carries from/to", () => {
    const ev = normalizeTimelineEvent({
      event: "renamed",
      actor: { login: "carol" },
      created_at: "2026-07-03T00:00:00Z",
      rename: { from: "old", to: "new" },
    });
    expect(ev?.detail?.from).toBe("old");
    expect(ev?.detail?.to).toBe("new");
  });

  test("merged: carries short commit sha", () => {
    const ev = normalizeTimelineEvent({
      event: "merged",
      actor: { login: "carol" },
      created_at: "2026-07-03T00:00:00Z",
      commit_id: "0123456789abcdef",
    });
    expect(ev?.detail?.sha).toBe("0123456");
  });
});

guarded("activity endpoint", () => {
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

  test("paginates via Link rel=next and filters unknown events", async () => {
    const app = createApp(
      deps(
        timelineOctokit([
          [
            { event: "mentioned" },
            { event: "labeled", actor: { login: "a" }, created_at: "t1", label: { name: "bug" } },
          ],
          [
            { event: "reviewed", state: "approved", user: { login: "b" }, submitted_at: "t2" },
            { event: "subscribed" },
          ],
        ]),
      ),
    );
    const res = await app.request(`${URL}?account=alpha`, { headers: A });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { items: { event: string }[] };
    expect(body.items.map((i) => i.event)).toEqual(["labeled", "reviewed"]);
  });

  test("rejects an account the caller does not own", async () => {
    const app = createApp(deps(timelineOctokit([[]])));
    const res = await app.request(`${URL}?account=ghost`, { headers: A });
    expect(res.status).toBe(403);
  });
});
