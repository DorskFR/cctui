import type { OpenAPIHono } from "@hono/zod-openapi";
import { upsertDocument } from "../db/documents.ts";
import type { AppDeps } from "../deps.ts";
import { verifySignature } from "../github/webhook.ts";

interface PullPayload {
  action?: string;
  number?: number;
  repository?: { owner?: { login?: string }; name?: string; full_name?: string };
  pull_request?: unknown;
}

export function registerWebhook(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openAPIRegistry.registerPath({
    method: "post",
    path: "/v1/webhook",
    summary: "GitHub webhook ingestion",
    description:
      "Optional push path for org repos that can install a webhook. Verifies " +
      "X-Hub-Signature-256 (HMAC-SHA256 of the raw body with the shared secret) and " +
      "upserts the payload exactly like a poll result. Polling remains the universal path.",
    tags: ["events"],
    responses: {
      202: { description: "Accepted and stored" },
      401: { description: "Missing or invalid signature" },
    },
  });

  app.post("/v1/webhook", async (c) => {
    const secret = deps.webhookSecret;
    if (!secret)
      return c.json({ error: { code: "not_configured", message: "No webhook secret" } }, 503);
    const raw = await c.req.text();
    const signature = c.req.header("x-hub-signature-256") ?? null;
    if (!verifySignature(secret, raw, signature)) {
      return c.json({ error: { code: "unauthorized", message: "Bad signature" } }, 401);
    }
    const event = c.req.header("x-github-event");
    const body = JSON.parse(raw) as PullPayload;

    if (deps.db && event === "pull_request" && body.pull_request) {
      const owner = body.repository?.owner?.login;
      const repo = body.repository?.name;
      const number = body.number;
      if (owner && repo && number) {
        await upsertDocument(deps.db, {
          account: owner,
          kind: "pull_request",
          key: `${owner}/${repo}#${number}`,
          etag: null,
          payload: body.pull_request,
        });
      }
    }
    return c.json({ ok: true }, 202);
  });
}
