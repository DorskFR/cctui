import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { listSnoozedPulls, snoozePull, unsnoozePull } from "../db/prSnooze.ts";
import type { PullRef } from "../db/viewedState.ts";
import type { AppDeps } from "../deps.ts";
import {
  ErrorSchema,
  SnoozedPullListSchema,
  SnoozeRequestSchema,
  SnoozeResultSchema,
} from "../schemas.ts";

const PullParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
  number: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "number", in: "path" }, example: 42 }),
});

const AccountQuery = z.object({
  account: z
    .string()
    .min(1)
    .openapi({ param: { name: "account", in: "query" }, example: "DorskFR" }),
});

const listSnoozed = createRoute({
  method: "get",
  path: "/v1/pulls/snoozed",
  summary: "List snoozed pull requests (excluded from the default list)",
  tags: ["pulls"],
  request: {
    query: z.object({
      account: z
        .string()
        .min(1)
        .optional()
        .openapi({ param: { name: "account", in: "query" }, example: "DorskFR" }),
    }),
  },
  responses: {
    200: {
      description: "The snoozed pull requests with their envelopes",
      content: { "application/json": { schema: SnoozedPullListSchema } },
    },
  },
});

const snooze = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/snooze",
  summary: "Snooze a pull request (hide it from the default list)",
  tags: ["pulls"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: SnoozeRequestSchema } } },
  },
  responses: {
    200: {
      description: "The snooze state",
      content: { "application/json": { schema: SnoozeResultSchema } },
    },
    503: {
      description: "State store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const unsnooze = createRoute({
  method: "delete",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/snooze",
  summary: "Un-snooze a pull request (return it to the default list)",
  tags: ["pulls"],
  request: { params: PullParams, query: AccountQuery },
  responses: {
    200: {
      description: "The snooze state",
      content: { "application/json": { schema: SnoozeResultSchema } },
    },
    503: {
      description: "State store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

function unavailable() {
  return { error: { code: "unavailable", message: "Snooze store is not configured" } } as const;
}

export function registerSnooze(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listSnoozed, async (c) => {
    const { account } = c.req.valid("query");
    if (!deps.db) return c.json({ items: [] }, 200);
    const userId = getUserId(c);
    const items = await listSnoozedPulls(deps.db, account, userId);
    return c.json({ items }, 200);
  });

  app.openapi(snooze, async (c) => {
    const { owner, repo, number } = c.req.valid("param");
    const { account } = c.req.valid("json");
    if (!deps.db) return c.json(unavailable(), 503);
    const ref: PullRef = { owner, repo, number };
    const userId = getUserId(c);
    const ok = await snoozePull(deps.db, account, ref, userId);
    return c.json({ account, owner, repo, number, snoozed: ok }, 200);
  });

  app.openapi(unsnooze, async (c) => {
    const { owner, repo, number } = c.req.valid("param");
    const { account } = c.req.valid("query");
    if (!deps.db) return c.json(unavailable(), 503);
    const ref: PullRef = { owner, repo, number };
    const userId = getUserId(c);
    await unsnoozePull(deps.db, account, ref, userId);
    return c.json({ account, owner, repo, number, snoozed: false }, 200);
  });
}
