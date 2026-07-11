import { createRoute, type OpenAPIHono } from "@hono/zod-openapi";
import { listDocuments } from "../db/documents.ts";
import type { AppDeps } from "../deps.ts";
import { NotificationPageSchema, PaginationQuerySchema } from "../schemas.ts";

const listNotifications = createRoute({
  method: "get",
  path: "/v1/notifications",
  summary: "List the notifications inbox",
  tags: ["notifications"],
  request: { query: PaginationQuerySchema },
  responses: {
    200: {
      description: "A page of notification envelopes",
      content: { "application/json": { schema: NotificationPageSchema } },
    },
  },
});

export function registerNotifications(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listNotifications, async (c) => {
    const { account, limit, cursor } = c.req.valid("query");
    if (!deps.db) return c.json({ items: [], next_cursor: null }, 200);
    const page = await listDocuments(deps.db, "notification", { account, limit, cursor });
    return c.json(page, 200);
  });
}
