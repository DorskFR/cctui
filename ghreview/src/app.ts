import { OpenAPIHono } from "@hono/zod-openapi";
import type { AppDeps } from "./deps.ts";
import { registerEvents } from "./routes/events.ts";
import { registerHealth } from "./routes/health.ts";
import { registerNotifications } from "./routes/notifications.ts";
import { registerPulls } from "./routes/pulls.ts";
import { registerRepos } from "./routes/repos.ts";
import { registerWebhook } from "./routes/webhook.ts";
import { version } from "./version.ts";

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

  registerHealth(app, deps);
  registerRepos(app, deps);
  registerPulls(app, deps);
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
