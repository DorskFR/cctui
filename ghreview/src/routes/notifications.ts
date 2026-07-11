import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import {
  applyNotificationState,
  listNotificationInbox,
  type NotificationStateItem,
} from "../db/notificationState.ts";
import type { AppDeps } from "../deps.ts";
import {
  ErrorSchema,
  NotificationBulkStateSchema,
  NotificationInboxPageSchema,
  NotificationInboxQuerySchema,
  NotificationSingleStateSchema,
  NotificationStateResultSchema,
} from "../schemas.ts";
import { pushThreadRead } from "../sync/notificationPush.ts";

const REASON_ALIASES: Record<string, string> = {
  "review-requested": "review_requested",
  ci: "ci_activity",
};

function toBool(v: "true" | "false" | undefined): boolean | undefined {
  return v === undefined ? undefined : v === "true";
}

const listNotifications = createRoute({
  method: "get",
  path: "/v1/notifications",
  summary: "The notifications inbox feed (documents + local state)",
  tags: ["notifications"],
  request: { query: NotificationInboxQuerySchema },
  responses: {
    200: {
      description: "A page of inbox items (envelope + read/done/archived state)",
      content: { "application/json": { schema: NotificationInboxPageSchema } },
    },
  },
});

const IdParam = z.object({
  id: z.string().openapi({ param: { name: "id", in: "path" }, example: "thread-42" }),
});

const bulkState = createRoute({
  method: "post",
  path: "/v1/notifications/state",
  summary: "Bulk mark notifications read/done/archived",
  tags: ["notifications"],
  request: {
    body: { content: { "application/json": { schema: NotificationBulkStateSchema } } },
  },
  responses: {
    200: {
      description: "The updated state for each thread",
      content: { "application/json": { schema: NotificationStateResultSchema } },
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

const singleState = createRoute({
  method: "post",
  path: "/v1/notifications/{id}/state",
  summary: "Mark one notification read/done/archived",
  tags: ["notifications"],
  request: {
    params: IdParam,
    body: { content: { "application/json": { schema: NotificationSingleStateSchema } } },
  },
  responses: {
    200: {
      description: "The updated state for the thread",
      content: { "application/json": { schema: NotificationStateResultSchema } },
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

async function pushReads(
  deps: AppDeps,
  account: string,
  items: NotificationStateItem[],
  read: boolean | undefined,
): Promise<void> {
  if (read !== true || !deps.db) return;
  const acct = deps.accountFor?.(account);
  if (!acct) return;
  for (const item of items) {
    const outcome = await pushThreadRead(deps.db, acct, item.thread_id);
    item.state.push_pending = !outcome.ok;
    item.state.last_error = outcome.ok ? null : (outcome.error ?? item.state.last_error);
  }
}

export function registerNotifications(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listNotifications, async (c) => {
    const q = c.req.valid("query");
    if (!deps.db) return c.json({ items: [], next_cursor: null }, 200);
    const userId = getUserId(c);
    const reason = q.reason ? (REASON_ALIASES[q.reason] ?? q.reason) : undefined;
    const page = await listNotificationInbox(deps.db, {
      account: q.account,
      limit: q.limit,
      cursor: q.cursor,
      reason,
      repo: q.repo,
      unread: toBool(q.unread),
      undone: toBool(q.undone),
      archived: toBool(q.archived),
      since: q.since,
      userId,
    });
    return c.json(page, 200);
  });

  app.openapi(bulkState, async (c) => {
    const body = c.req.valid("json");
    if (!deps.db) {
      return c.json(
        { error: { code: "unavailable", message: "Notification state store is not configured" } },
        503,
      );
    }
    const userId = getUserId(c);
    const items = await applyNotificationState(
      deps.db,
      body.account,
      body.thread_ids,
      { read: body.read, done: body.done, archived: body.archived },
      userId,
    );
    await pushReads(deps, body.account, items, body.read);
    return c.json({ items }, 200);
  });

  app.openapi(singleState, async (c) => {
    const { id } = c.req.valid("param");
    const body = c.req.valid("json");
    if (!deps.db) {
      return c.json(
        { error: { code: "unavailable", message: "Notification state store is not configured" } },
        503,
      );
    }
    const userId = getUserId(c);
    const items = await applyNotificationState(
      deps.db,
      body.account,
      [id],
      { read: body.read, done: body.done, archived: body.archived },
      userId,
    );
    await pushReads(deps, body.account, items, body.read);
    return c.json({ items }, 200);
  });
}
