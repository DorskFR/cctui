import { createRoute, type OpenAPIHono } from "@hono/zod-openapi";
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

export function registerNotifications(app: OpenAPIHono) {
  app.openapi(listNotifications, (c) => c.json({ items: [], next_cursor: null }, 200));
}
