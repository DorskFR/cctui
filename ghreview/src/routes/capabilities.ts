import { createRoute, type OpenAPIHono } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { countGhAccounts } from "../db/accounts.ts";
import { listUserRepoSlugs } from "../db/subscriptions.ts";
import type { AppDeps } from "../deps.ts";
import { CapabilitiesSchema } from "../schemas.ts";

const OFF = { github: { available: false, enabled: false, repos: [] as string[] } };

const capabilities = createRoute({
  method: "get",
  path: "/v1/capabilities",
  summary: "GitHub integration capability, derived from the caller's connector state",
  tags: ["system"],
  responses: {
    200: {
      description:
        "available: store reachable (gates nav/unlock); enabled: caller has ≥1 connector; " +
        "repos: distinct tracked repo slugs",
      content: { "application/json": { schema: CapabilitiesSchema } },
    },
  },
});

export function registerCapabilities(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(capabilities, async (c) => {
    if (!deps.db) return c.json(OFF, 200);
    const uid = getUserId(c) ?? "";
    try {
      const count = await countGhAccounts(deps.db, uid);
      const repos = count > 0 ? await listUserRepoSlugs(deps.db, uid) : [];
      return c.json({ github: { available: true, enabled: count > 0, repos } }, 200);
    } catch {
      return c.json(OFF, 200);
    }
  });
}
