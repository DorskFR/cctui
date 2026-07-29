import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { listUserLogins } from "../db/accounts.ts";
import type { AppDeps } from "../deps.ts";
import { StatusSchema } from "../schemas.ts";
import { version } from "../version.ts";

const healthRoute = createRoute({
  method: "get",
  path: "/v1/health",
  summary: "Liveness probe",
  tags: ["system"],
  responses: {
    200: {
      description: "Service is up",
      content: { "application/json": { schema: z.object({ ok: z.literal(true) }) } },
    },
  },
});

const statusRoute = createRoute({
  method: "get",
  path: "/v1/status",
  summary: "Service and sync status",
  tags: ["system"],
  responses: {
    200: {
      description: "Current service and sync status",
      content: { "application/json": { schema: StatusSchema } },
    },
  },
});

export function registerHealth(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(healthRoute, (c) => c.json({ ok: true as const }, 200));
  app.openapi(statusRoute, async (c) => {
    const snap = deps.syncSnapshot?.() ?? { last_run: null, accounts: [] as string[] };
    const uid = getUserId(c);
    let accounts: string[] = [];
    if (deps.db && uid) {
      const mine = new Set(await listUserLogins(deps.db, uid));
      accounts = snap.accounts.filter((a) => mine.has(a));
    }
    return c.json(
      {
        service: "gh-review" as const,
        version,
        api: "v1" as const,
        ok: true,
        sync: { last_run: snap.last_run, accounts },
      },
      200,
    );
  });
}
