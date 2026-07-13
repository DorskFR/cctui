import { OpenAPIHono } from "@hono/zod-openapi";
import { authMiddleware } from "./auth/middleware.ts";
import type { AppDeps } from "./deps.ts";
import { registerAccounts } from "./routes/accounts.ts";
import { registerEvents } from "./routes/events.ts";
import { registerHealth } from "./routes/health.ts";
import { registerLabels } from "./routes/labels.ts";
import { registerNotifications } from "./routes/notifications.ts";
import { registerPulls } from "./routes/pulls.ts";
import { registerReactions } from "./routes/reactions.ts";
import { registerRepos } from "./routes/repos.ts";
import { registerReviews } from "./routes/reviews.ts";
import { registerSubscriptions } from "./routes/subscriptions.ts";
import { registerViewed } from "./routes/viewed.ts";
import { registerWebhook } from "./routes/webhook.ts";
import { version } from "./version.ts";

const AUTH_EXEMPT = new Set(["/v1/health", "/v1/status", "/v1/webhook", "/v1/openapi.json"]);

export function createApp(deps: AppDeps = {}) {
  const app = new OpenAPIHono({
    defaultHook: (result, c) => {
      if (!result.success) {
        return c.json(
          {
            error: {
              code: "invalid_request",
              message: "Request validation failed",
              details: result.error.issues,
            },
          },
          400,
        );
      }
    },
  });

  if (deps.auth) {
    const guard = authMiddleware(deps.auth);
    app.use("/v1/*", async (c, next) => {
      if (AUTH_EXEMPT.has(c.req.path)) return next();
      return guard(c, next);
    });
  }

  registerHealth(app, deps);
  registerAccounts(app, deps);
  registerRepos(app, deps);
  registerSubscriptions(app, deps);
  registerPulls(app, deps);
  registerViewed(app, deps);
  registerReviews(app, deps);
  registerReactions(app, deps);
  registerLabels(app, deps);
  registerNotifications(app, deps);
  registerEvents(app, deps);
  registerWebhook(app, deps);

  app.notFound((c) => c.json({ error: { code: "not_found", message: "No such route" } }, 404));
  app.onError((err, c) => c.json({ error: { code: "internal", message: err.message } }, 500));

  app.doc("/v1/openapi.json", {
    openapi: "3.0.3",
    info: {
      title: "cctui gh-review API",
      version,
      description:
        "Versioned HTTP+SSE contract for the GitHub review center. Relays GitHub-shaped " +
        "JSONB payloads verbatim inside typed envelopes.",
    },
    servers: [{ url: "/" }],
  });

  return app;
}
