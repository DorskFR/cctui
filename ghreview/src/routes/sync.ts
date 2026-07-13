import { createRoute, type OpenAPIHono } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { listUserLogins } from "../db/accounts.ts";
import type { AppDeps } from "../deps.ts";
import { ErrorSchema, SyncRequestSchema, SyncResultSchema } from "../schemas.ts";

const forceSync = createRoute({
  method: "post",
  path: "/v1/sync",
  summary: "Run an immediate incremental poll cycle for an account",
  tags: ["system"],
  request: { body: { content: { "application/json": { schema: SyncRequestSchema } } } },
  responses: {
    200: {
      description: "Incremental sync completed",
      content: { "application/json": { schema: SyncResultSchema } },
    },
    400: {
      description: "Account required (caller owns more than one)",
      content: { "application/json": { schema: ErrorSchema } },
    },
    404: {
      description: "No such account owned by the caller",
      content: { "application/json": { schema: ErrorSchema } },
    },
    409: {
      description: "A forced sync is already running for this account",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store/sync unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

export function registerSync(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(forceSync, async (c) => {
    if (!deps.db || !deps.forceSync)
      return c.json({ error: { code: "unavailable", message: "Sync not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const body = c.req.valid("json");

    const logins = await listUserLogins(deps.db, uid);
    let account = body.account;
    if (!account) {
      if (logins.length === 1) account = logins[0];
      else if (logins.length === 0)
        return c.json(
          { error: { code: "not_found", message: "No GitHub account configured" } },
          404,
        );
      else
        return c.json(
          { error: { code: "account_required", message: "Specify which account to sync" } },
          400,
        );
    }
    if (!logins.includes(account as string))
      return c.json(
        { error: { code: "not_found", message: `Account ${account} is not owned by the caller` } },
        404,
      );

    const result = await deps.forceSync(account as string);
    if (result === "unknown")
      return c.json(
        { error: { code: "not_found", message: `Account ${account} is not active` } },
        404,
      );
    if (result === "busy")
      return c.json(
        { error: { code: "sync_in_progress", message: "A forced sync is already running" } },
        409,
      );
    return c.json({ account: account as string, status: "ok" as const }, 200);
  });
}
