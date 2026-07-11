import type { OpenAPIHono } from "@hono/zod-openapi";
import { z } from "@hono/zod-openapi";
import { streamSSE } from "hono/streaming";
import { AccountSchema } from "../schemas.ts";

export const PrUpdatedEventSchema = z
  .object({
    event: z.literal("pr.updated"),
    data: z.object({
      account: AccountSchema,
      owner: z.string(),
      repo: z.string(),
      number: z.number().int().positive(),
    }),
  })
  .openapi("PrUpdatedEvent");

export const NotificationNewEventSchema = z
  .object({
    event: z.literal("notification.new"),
    data: z.object({ account: AccountSchema, id: z.string() }),
  })
  .openapi("NotificationNewEvent");

export const SyncStatusEventSchema = z
  .object({
    event: z.literal("sync.status"),
    data: z.object({
      account: AccountSchema,
      state: z.enum(["idle", "syncing", "error"]),
      last_run: z.string().datetime().nullable(),
    }),
  })
  .openapi("SyncStatusEvent");

export const SseEventSchema = z
  .discriminatedUnion("event", [
    PrUpdatedEventSchema,
    NotificationNewEventSchema,
    SyncStatusEventSchema,
  ])
  .openapi("SseEvent");

export function registerEvents(app: OpenAPIHono) {
  app.openAPIRegistry.register("SseEvent", SseEventSchema);
  app.openAPIRegistry.registerPath({
    method: "get",
    path: "/v1/events",
    summary: "Server-Sent Events stream",
    description:
      "text/event-stream of the documented event catalogue. Each message carries an `event:` " +
      "name (pr.updated, notification.new, sync.status) and a JSON `data:` payload matching SseEvent.",
    tags: ["events"],
    responses: {
      200: {
        description: "An SSE stream of SseEvent messages",
        content: {
          "text/event-stream": {
            schema: { $ref: "#/components/schemas/SseEvent" },
          },
        },
      },
    },
  });

  app.get("/v1/events", (c) =>
    streamSSE(c, async (stream) => {
      await stream.writeSSE({ event: "sync.status", data: JSON.stringify({ ready: true }) });
      while (!stream.closed) {
        await stream.writeSSE({ event: "ping", data: "" });
        await stream.sleep(15_000);
      }
    }),
  );
}
