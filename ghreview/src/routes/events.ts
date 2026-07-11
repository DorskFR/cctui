import type { OpenAPIHono } from "@hono/zod-openapi";
import { z } from "@hono/zod-openapi";
import { streamSSE } from "hono/streaming";
import { getUserId } from "../auth/middleware.ts";
import { listUserLogins } from "../db/accounts.ts";
import type { AppDeps } from "../deps.ts";
import type { SseMessage } from "../events/bus.ts";
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

export const NotificationUpdatedEventSchema = z
  .object({
    event: z.literal("notification.updated"),
    data: z.object({ account: AccountSchema, id: z.string() }),
  })
  .openapi("NotificationUpdatedEvent");

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
    NotificationUpdatedEventSchema,
    SyncStatusEventSchema,
  ])
  .openapi("SseEvent");

export function registerEvents(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openAPIRegistry.register("SseEvent", SseEventSchema);
  app.openAPIRegistry.registerPath({
    method: "get",
    path: "/v1/events",
    summary: "Server-Sent Events stream",
    description:
      "text/event-stream of the documented event catalogue. Each message carries an `event:` " +
      "name (pr.updated, notification.new, notification.updated, sync.status) and a JSON `data:` " +
      "payload matching SseEvent.",
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

  app.get("/v1/events", async (c) => {
    const userId = getUserId(c);
    let allowed: Set<string> | null = null;
    if (userId && deps.db) {
      allowed = new Set(await listUserLogins(deps.db, userId));
    }
    const visible = (msg: SseMessage): boolean => {
      if (!allowed) return true;
      const account = (msg.data as { account?: string } | undefined)?.account;
      return account === undefined || allowed.has(account);
    };
    return streamSSE(c, async (stream) => {
      const queue: SseMessage[] = [];
      const unsubscribe = deps.bus?.subscribe((msg) => queue.push(msg));
      await stream.writeSSE({ event: "sync.status", data: JSON.stringify({ ready: true }) });
      try {
        while (!stream.closed) {
          let msg = queue.shift();
          while (msg) {
            if (visible(msg)) {
              await stream.writeSSE({ event: msg.event, data: JSON.stringify(msg.data) });
            }
            msg = queue.shift();
          }
          await stream.writeSSE({ event: "ping", data: "" });
          await stream.sleep(1_000);
        }
      } finally {
        unsubscribe?.();
      }
    });
  });
}
