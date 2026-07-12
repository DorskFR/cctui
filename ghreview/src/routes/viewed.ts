import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { getDocument } from "../db/documents.ts";
import {
  applyViewedState,
  listViewedState,
  type PullRef,
  type ViewedStateItem,
} from "../db/viewedState.ts";
import type { AppDeps } from "../deps.ts";
import { ErrorSchema, ViewedStateResultSchema, ViewedStateSetSchema } from "../schemas.ts";
import { pushViewedFile, resolvePullNodeId } from "../sync/viewedPush.ts";
import { digestPullFiles } from "../sync/viewedSync.ts";

const PullParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
  number: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "number", in: "path" }, example: 42 }),
});

const getViewed = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/viewed",
  summary: "Per-file viewed state for a pull request",
  tags: ["pulls"],
  request: {
    params: PullParams,
    query: z.object({
      account: z
        .string()
        .min(1)
        .openapi({ param: { name: "account", in: "query" }, example: "DorskFR" }),
    }),
  },
  responses: {
    200: {
      description: "The viewed state per marked file",
      content: { "application/json": { schema: ViewedStateResultSchema } },
    },
  },
});

const setViewed = createRoute({
  method: "put",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/viewed",
  summary: "Bulk set per-file viewed state (single file or a whole folder)",
  tags: ["pulls"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: ViewedStateSetSchema } } },
  },
  responses: {
    200: {
      description: "The updated viewed state per file",
      content: { "application/json": { schema: ViewedStateResultSchema } },
    },
    400: {
      description: "Invalid request",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "State store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

async function pushViewed(
  deps: AppDeps,
  account: string,
  ref: PullRef,
  items: ViewedStateItem[],
): Promise<void> {
  if (!deps.db || items.length === 0) return;
  const acct = deps.accountFor?.(account);
  if (!acct) return;
  const nodeId = await resolvePullNodeId(deps.db, account, ref);
  for (const item of items) {
    const outcome = await pushViewedFile(deps.db, acct, ref, item.path, item.viewed, nodeId);
    item.push_pending = !outcome.ok;
    item.last_error = outcome.ok ? null : (outcome.error ?? item.last_error);
  }
}

export function registerViewed(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(getViewed, async (c) => {
    const { owner, repo, number } = c.req.valid("param");
    const { account } = c.req.valid("query");
    if (!deps.db) return c.json({ items: [] }, 200);
    const userId = getUserId(c);
    const items = await listViewedState(deps.db, account, { owner, repo, number }, userId);
    return c.json({ items }, 200);
  });

  app.openapi(setViewed, async (c) => {
    const { owner, repo, number } = c.req.valid("param");
    const body = c.req.valid("json");
    if (!deps.db) {
      return c.json(
        { error: { code: "unavailable", message: "Viewed state store is not configured" } },
        503,
      );
    }
    const ref: PullRef = { owner, repo, number };
    const userId = getUserId(c);
    const doc = await getDocument(
      deps.db,
      body.account,
      "pull_request",
      `${owner}/${repo}#${number}`,
    );
    const digestByPath = digestPullFiles(doc?.payload);
    const items = await applyViewedState(
      deps.db,
      body.account,
      ref,
      body.paths,
      body.viewed,
      digestByPath,
      userId,
    );
    await pushViewed(deps, body.account, ref, items);
    return c.json({ items }, 200);
  });
}
