import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { listGhAccounts } from "../db/accounts.ts";
import {
  deactivateOwnedSubscription,
  listSubscriptionsForUser,
  type SubscriptionKind,
  upsertOwnedSubscription,
} from "../db/subscriptions.ts";
import type { AppDeps } from "../deps.ts";
import {
  ErrorSchema,
  SubscriptionCreateSchema,
  SubscriptionListSchema,
  SubscriptionSchema,
} from "../schemas.ts";
import { syncPull } from "../sync/handlers.ts";

const PULL_URL = /^https?:\/\/github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/i;
const PULL_SHORT = /^([^/\s]+)\/([^/#\s]+)#(\d+)$/;
const REPO_URL = /^https?:\/\/github\.com\/([^/]+)\/([^/]+?)(?:\.git)?\/?$/i;
const REPO_SHORT = /^([^/\s]+)\/([^/\s]+)$/;

function normalizeTarget(kind: SubscriptionKind, raw: string): string | null {
  const target = raw.trim();
  if (kind === "notification") return null;
  if (kind === "pull_request") {
    const url = PULL_URL.exec(target);
    if (url) return `${url[1]}/${url[2]}#${url[3]}`;
    const short = PULL_SHORT.exec(target);
    if (short) return `${short[1]}/${short[2]}#${short[3]}`;
    return null;
  }
  const url = REPO_URL.exec(target);
  if (url) return `${url[1]}/${url[2]}`;
  const short = REPO_SHORT.exec(target);
  if (short) return `${short[1]}/${short[2]}`;
  return null;
}

const IdParam = z.object({
  id: z.string().openapi({ param: { name: "id", in: "path" }, example: "1" }),
});

const AccountQuery = z.object({
  account: z
    .string()
    .min(1)
    .optional()
    .openapi({
      param: { name: "account", in: "query" },
      description: "Filter to a single account",
    }),
});

const listSubscriptions = createRoute({
  method: "get",
  path: "/v1/subscriptions",
  summary: "List the caller's active subscriptions",
  tags: ["subscriptions"],
  request: { query: AccountQuery },
  responses: {
    200: {
      description: "The caller's active subscriptions",
      content: { "application/json": { schema: SubscriptionListSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const createSubscription = createRoute({
  method: "post",
  path: "/v1/subscriptions",
  summary: "Subscribe to a PR (URL or owner/repo#n) or a repo",
  tags: ["subscriptions"],
  request: { body: { content: { "application/json": { schema: SubscriptionCreateSchema } } } },
  responses: {
    201: {
      description: "The created (or reactivated) subscription",
      content: { "application/json": { schema: SubscriptionSchema } },
    },
    400: {
      description: "Invalid target",
      content: { "application/json": { schema: ErrorSchema } },
    },
    404: {
      description: "No such account owned by the caller",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const deleteSubscription = createRoute({
  method: "delete",
  path: "/v1/subscriptions/{id}",
  summary: "Unsubscribe (deactivate) one of the caller's subscriptions",
  tags: ["subscriptions"],
  request: { params: IdParam },
  responses: {
    204: { description: "Deactivated" },
    404: { description: "Not found", content: { "application/json": { schema: ErrorSchema } } },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

export function registerSubscriptions(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listSubscriptions, async (c) => {
    if (!deps.db)
      return c.json({ error: { code: "unavailable", message: "Store not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const { account } = c.req.valid("query");
    const items = await listSubscriptionsForUser(deps.db, uid, account);
    return c.json({ items }, 200);
  });

  app.openapi(createSubscription, async (c) => {
    if (!deps.db)
      return c.json({ error: { code: "unavailable", message: "Store not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const body = c.req.valid("json");
    const kind = body.kind;

    const target = normalizeTarget(kind, body.target);
    if (kind !== "notification" && !target) {
      return c.json(
        { error: { code: "invalid_target", message: `Could not parse ${kind} target` } },
        400,
      );
    }

    let account = body.account;
    if (!account) {
      const accounts = await listGhAccounts(deps.db, uid);
      if (accounts.length === 1) account = accounts[0]?.login;
      else if (accounts.length === 0) {
        return c.json(
          { error: { code: "not_found", message: "No GitHub account configured" } },
          404,
        );
      } else {
        return c.json(
          {
            error: {
              code: "account_required",
              message: "Specify which account owns the subscription",
            },
          },
          400,
        );
      }
    }

    const row = await upsertOwnedSubscription(deps.db, uid, account as string, kind, target);
    if (!row) {
      return c.json(
        { error: { code: "not_found", message: `Account ${account} is not owned by the caller` } },
        404,
      );
    }

    if (row.kind === "pull_request") {
      const acct = deps.accountFor?.(row.account);
      if (acct) await syncPull({ db: deps.db, account: acct }, row).catch(() => {});
    }

    return c.json(row, 201);
  });

  app.openapi(deleteSubscription, async (c) => {
    if (!deps.db)
      return c.json({ error: { code: "unavailable", message: "Store not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const { id } = c.req.valid("param");
    const row = await deactivateOwnedSubscription(deps.db, uid, id);
    if (!row) {
      return c.json({ error: { code: "not_found", message: `Subscription ${id} not found` } }, 404);
    }
    return c.body(null, 204);
  });
}
