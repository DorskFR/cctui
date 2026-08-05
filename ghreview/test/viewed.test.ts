import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import {
  applyGithubViewedState,
  applyViewedState,
  invalidateChangedViewed,
  listViewedState,
} from "../src/db/viewedState.ts";
import { createAccount } from "../src/github/account.ts";
import { pushFileViewed } from "../src/github/viewedFiles.ts";
import type { GraphqlClient } from "../src/graphql/client.ts";
import { drainPendingViewed } from "../src/sync/viewedPush.ts";
import { digestPullFiles } from "../src/sync/viewedSync.ts";
import { dbGate } from "./dbGate.ts";

function mockGraphql(overrides: Partial<GraphqlClient> = {}): GraphqlClient {
  const ok = async () => ({}) as never;
  return {
    reviewThreads: ok,
    pullViewedFiles: ok,
    markFileViewed: ok,
    unmarkFileViewed: ok,
    ...overrides,
  };
}

describe("pushFileViewed (transport)", () => {
  test("mark calls markFileViewed and returns ok", async () => {
    const calls: string[] = [];
    const gql = mockGraphql({
      markFileViewed: async () => {
        calls.push("mark");
        return {} as never;
      },
    });
    const res = await pushFileViewed(gql, "PR_1", "a.ts", true);
    expect(res.ok).toBe(true);
    expect(calls).toEqual(["mark"]);
  });

  test("unmark calls unmarkFileViewed", async () => {
    const calls: string[] = [];
    const gql = mockGraphql({
      unmarkFileViewed: async () => {
        calls.push("unmark");
        return {} as never;
      },
    });
    const res = await pushFileViewed(gql, "PR_1", "a.ts", false);
    expect(res.ok).toBe(true);
    expect(calls).toEqual(["unmark"]);
  });

  test("a NOT_FOUND graphql error is terminal-ok", async () => {
    const gql = mockGraphql({
      markFileViewed: async () => {
        throw { errors: [{ type: "NOT_FOUND", message: "gone" }] };
      },
    });
    expect((await pushFileViewed(gql, "PR_1", "a.ts", true)).ok).toBe(true);
  });

  test("a 500-shaped error is a failure", async () => {
    const gql = mockGraphql({
      markFileViewed: async () => {
        throw { status: 500, message: "boom" };
      },
    });
    const res = await pushFileViewed(gql, "PR_1", "a.ts", true);
    expect(res.ok).toBe(false);
    expect(res.error).toContain("boom");
  });
});

describe("digestPullFiles", () => {
  test("prefers blob sha, falls back to a patch digest", () => {
    const map = digestPullFiles({
      files: [
        { filename: "a.ts", sha: "sha-a", patch: "@@ -1 +1 @@" },
        { filename: "b.ts", patch: "@@ -2 +2 @@\n+x" },
        { filename: "c.ts" },
      ],
    });
    expect(map.get("a.ts")).toBe("sha-a");
    expect(map.get("b.ts")).toMatch(/^p:/);
    expect(map.has("c.ts")).toBe(false);
  });
});

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = dbGate(describe, DATABASE_URL);

let db: DbHandle;
const REF = { owner: "DorskFR", repo: "cctui", number: 7 };

async function seedPull(account: string, nodeId: string): Promise<void> {
  await upsertDocument(db, {
    account,
    kind: "pull_request",
    key: `${REF.owner}/${REF.repo}#${REF.number}`,
    etag: null,
    payload: {
      number: REF.number,
      node_id: nodeId,
      files: [
        { filename: "src/a.ts", sha: "sha-a" },
        { filename: "src/b.ts", sha: "sha-b" },
      ],
    },
  });
}

guarded("viewed state store", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
    await db.sql.unsafe(
      "INSERT INTO gh_accounts (user_id, login, encrypted_pat) VALUES ('u1', 'vb', 'x') ON CONFLICT (login) DO NOTHING",
    );
  });

  afterAll(async () => {
    if (db) await db.close();
  });

  beforeEach(async () => {
    await db.sql.unsafe(
      "DELETE FROM viewed_state WHERE account = 'vb'; DELETE FROM documents WHERE account = 'vb'",
    );
    await seedPull("vb", "PR_NODE_1");
  });

  test("set + read back viewed state", async () => {
    const digest = new Map([["src/a.ts", "sha-a"]]);
    await applyViewedState(db, "vb", REF, ["src/a.ts"], true, digest, "u1");
    const items = await listViewedState(db, "vb", REF, "u1");
    expect(items.length).toBe(1);
    const item = items[0] as (typeof items)[number];
    expect(item).toMatchObject({ path: "src/a.ts", viewed: true, push_pending: true });
    expect(item.digest).toBe("sha-a");
  });

  test("ownership: a foreign user sees nothing and cannot mutate", async () => {
    await applyViewedState(db, "vb", REF, ["src/a.ts"], true, new Map(), "u1");
    expect((await listViewedState(db, "vb", REF, "intruder")).length).toBe(0);
    const wrote = await applyViewedState(db, "vb", REF, ["src/b.ts"], true, new Map(), "intruder");
    expect(wrote.length).toBe(0);
    expect((await listViewedState(db, "vb", REF, "u1")).length).toBe(1);
  });

  test("invalidation clears viewed when the digest changes", async () => {
    await applyViewedState(
      db,
      "vb",
      REF,
      ["src/a.ts", "src/b.ts"],
      true,
      digestPullFiles({
        files: [
          { filename: "src/a.ts", sha: "sha-a" },
          { filename: "src/b.ts", sha: "sha-b" },
        ],
      }),
      "u1",
    );
    const cleared = await invalidateChangedViewed(
      db,
      "vb",
      REF,
      new Map([
        ["src/a.ts", "sha-a"],
        ["src/b.ts", "sha-b-NEW"],
      ]),
    );
    expect(cleared).toEqual(["src/b.ts"]);
    const items = await listViewedState(db, "vb", REF, "u1");
    const byPath = Object.fromEntries(items.map((i) => [i.path, i.viewed]));
    expect(byPath["src/a.ts"]).toBe(true);
    expect(byPath["src/b.ts"]).toBe(false);
  });

  test("github pull-in sets viewed without owing a push", async () => {
    const changed = await applyGithubViewedState(db, "vb", REF, new Map([["src/a.ts", true]]));
    expect(changed).toEqual(["src/a.ts"]);
    const row = (await listViewedState(db, "vb", REF, "u1"))[0];
    expect(row).toMatchObject({ viewed: true, push_pending: false });
  });

  test("github pull-in does not insert not-viewed rows for unmarked files", async () => {
    const changed = await applyGithubViewedState(
      db,
      "vb",
      REF,
      new Map([
        ["src/a.ts", false],
        ["src/b.ts", false],
      ]),
    );
    expect(changed).toEqual([]);
    expect((await listViewedState(db, "vb", REF, "u1")).length).toBe(0);
  });

  test("github pull-in does not clobber a local change still owed a push", async () => {
    await applyViewedState(db, "vb", REF, ["src/a.ts"], true, new Map(), "u1");
    const changed = await applyGithubViewedState(db, "vb", REF, new Map([["src/a.ts", false]]));
    expect(changed).toEqual([]);
    const row = (await listViewedState(db, "vb", REF, "u1"))[0];
    expect(row?.viewed).toBe(true);
  });

  test("drain pushes pending viewed changes, retries on failure", async () => {
    await applyViewedState(db, "vb", REF, ["src/a.ts"], true, new Map(), "u1");

    const failing = createAccount({
      login: "vb",
      token: undefined,
      graphql: mockGraphql({
        markFileViewed: async () => {
          throw { status: 500, message: "nope" };
        },
      }),
    });
    await drainPendingViewed(db, failing);
    let row = (await listViewedState(db, "vb", REF, "u1"))[0];
    expect(row?.push_pending).toBe(true);
    expect(row?.last_error).toContain("nope");

    const seen: { id: string; path: string }[] = [];
    const succeeding = createAccount({
      login: "vb",
      token: undefined,
      graphql: mockGraphql({
        markFileViewed: async (vars) => {
          seen.push({ id: String(vars.pullRequestId), path: vars.path });
          return {} as never;
        },
      }),
    });
    await drainPendingViewed(db, succeeding);
    row = (await listViewedState(db, "vb", REF, "u1"))[0];
    expect(row?.push_pending).toBe(false);
    expect(row?.last_error).toBeNull();
    expect(seen).toEqual([{ id: "PR_NODE_1", path: "src/a.ts" }]);
  });

  test("PUT route marks a folder's files and records digests", async () => {
    const auth = createStaticResolver(parseStaticTokens("tokU1:u1"));
    const app = createApp({ db, auth });
    const res = await app.request(`/v1/repos/${REF.owner}/${REF.repo}/pulls/${REF.number}/viewed`, {
      method: "PUT",
      headers: { authorization: "Bearer tokU1", "content-type": "application/json" },
      body: JSON.stringify({ account: "vb", paths: ["src/a.ts", "src/b.ts"], viewed: true }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { items: { path: string; digest: string | null }[] };
    expect(body.items.map((i) => i.path).sort()).toEqual(["src/a.ts", "src/b.ts"]);
    expect(body.items.find((i) => i.path === "src/a.ts")?.digest).toBe("sha-a");
  });
});
