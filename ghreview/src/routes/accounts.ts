import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import {
  AccountConflictError,
  createGhAccount,
  deleteGhAccount,
  getGhAccount,
  listGhAccounts,
  updateGhAccount,
} from "../db/accounts.ts";
import { upsertSubscription } from "../db/subscriptions.ts";
import type { AppDeps } from "../deps.ts";
import { validatePat } from "../github/validate.ts";
import {
  AccountCreateSchema,
  AccountListSchema,
  AccountSummarySchema,
  AccountUpdateSchema,
  ErrorSchema,
} from "../schemas.ts";

const IdParam = z.object({
  id: z.string().openapi({ param: { name: "id", in: "path" }, example: "1" }),
});

const DEFAULT_LIMITS = { rateLimitPerHour: 5000, pollIntervalMs: 30_000 };

function clampRateLimit<T extends number | null | undefined>(deps: AppDeps, value: T): T {
  if (value == null) return value;
  const max = deps.limits?.rateLimitPerHour ?? DEFAULT_LIMITS.rateLimitPerHour;
  return Math.min(value, max) as T;
}

function clampPollInterval<T extends number | null | undefined>(deps: AppDeps, value: T): T {
  if (value == null) return value;
  const min = deps.limits?.pollIntervalMs ?? DEFAULT_LIMITS.pollIntervalMs;
  return Math.max(value, min) as T;
}

const listAccounts = createRoute({
  method: "get",
  path: "/v1/accounts",
  summary: "List the caller's GitHub accounts (no secrets)",
  tags: ["accounts"],
  responses: {
    200: {
      description: "The caller's accounts",
      content: { "application/json": { schema: AccountListSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const createAccount = createRoute({
  method: "post",
  path: "/v1/accounts",
  summary: "Add a GitHub account (validates the PAT, seals it, never returns it)",
  tags: ["accounts"],
  request: { body: { content: { "application/json": { schema: AccountCreateSchema } } } },
  responses: {
    201: {
      description: "The created account (no secrets)",
      content: { "application/json": { schema: AccountSummarySchema } },
    },
    400: { description: "Invalid PAT", content: { "application/json": { schema: ErrorSchema } } },
    409: {
      description: "Login already owned",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store/sealer unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const patchAccount = createRoute({
  method: "patch",
  path: "/v1/accounts/{id}",
  summary: "Update a connector's poll knobs, or rotate its PAT (never returns the secret)",
  tags: ["accounts"],
  request: {
    params: IdParam,
    body: { content: { "application/json": { schema: AccountUpdateSchema } } },
  },
  responses: {
    200: {
      description: "The updated account (no secrets)",
      content: { "application/json": { schema: AccountSummarySchema } },
    },
    400: { description: "Invalid PAT", content: { "application/json": { schema: ErrorSchema } } },
    404: { description: "Not found", content: { "application/json": { schema: ErrorSchema } } },
    503: {
      description: "Store/sealer unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const deleteAccount = createRoute({
  method: "delete",
  path: "/v1/accounts/{id}",
  summary: "Remove one of the caller's GitHub accounts",
  tags: ["accounts"],
  request: { params: IdParam },
  responses: {
    204: { description: "Deleted" },
    404: { description: "Not found", content: { "application/json": { schema: ErrorSchema } } },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

export function registerAccounts(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listAccounts, async (c) => {
    if (!deps.db)
      return c.json({ error: { code: "unavailable", message: "Store not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const items = await listGhAccounts(deps.db, uid);
    return c.json({ items }, 200);
  });

  app.openapi(createAccount, async (c) => {
    const body = c.req.valid("json");
    if (!deps.db || !deps.sealer) {
      return c.json(
        { error: { code: "unavailable", message: "Store or sealer not configured" } },
        503,
      );
    }
    const uid = getUserId(c) ?? "";
    const validate = deps.validatePat ?? ((t: string) => validatePat(t, deps.octokitForPat));
    const result = await validate(body.token);
    if (!result.ok || !result.login) {
      return c.json(
        {
          error: {
            code: "invalid_pat",
            message: `GitHub rejected the PAT (status ${result.status})`,
          },
        },
        400,
      );
    }
    if (body.login && body.login !== result.login) {
      return c.json(
        {
          error: {
            code: "login_mismatch",
            message: `PAT belongs to ${result.login}, not ${body.login}`,
          },
        },
        400,
      );
    }
    try {
      const account = await createGhAccount(deps.db, {
        userId: uid,
        login: result.login,
        encryptedPat: deps.sealer.seal(body.token),
        pollIntervalMs: clampPollInterval(deps, body.poll_interval_ms) ?? null,
        budgetCeiling: body.budget_ceiling ?? null,
        rateLimit: clampRateLimit(deps, body.rate_limit) ?? null,
      });
      await upsertSubscription(deps.db, result.login, "notification", null, "notification").catch(
        () => {},
      );
      return c.json(account, 201);
    } catch (err) {
      if (err instanceof AccountConflictError) {
        return c.json({ error: { code: "conflict", message: err.message } }, 409);
      }
      throw err;
    }
  });

  app.openapi(patchAccount, async (c) => {
    const { id } = c.req.valid("param");
    const body = c.req.valid("json");
    if (!deps.db)
      return c.json({ error: { code: "unavailable", message: "Store not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const existing = await getGhAccount(deps.db, uid, id);
    if (!existing) {
      return c.json({ error: { code: "not_found", message: `Account ${id} not found` } }, 404);
    }
    let encryptedPat: string | undefined;
    if (body.token !== undefined) {
      if (!deps.sealer) {
        return c.json({ error: { code: "unavailable", message: "Sealer not configured" } }, 503);
      }
      const validate = deps.validatePat ?? ((t: string) => validatePat(t, deps.octokitForPat));
      const result = await validate(body.token);
      if (!result.ok || !result.login) {
        return c.json(
          {
            error: {
              code: "invalid_pat",
              message: `GitHub rejected the PAT (status ${result.status})`,
            },
          },
          400,
        );
      }
      if (result.login !== existing.login) {
        return c.json(
          {
            error: {
              code: "login_mismatch",
              message: `PAT belongs to ${result.login}, not ${existing.login}`,
            },
          },
          400,
        );
      }
      encryptedPat = deps.sealer.seal(body.token);
    }
    const updated = await updateGhAccount(deps.db, uid, id, {
      pollIntervalMs: clampPollInterval(deps, body.poll_interval_ms),
      budgetCeiling: body.budget_ceiling,
      rateLimit: clampRateLimit(deps, body.rate_limit),
      encryptedPat,
    });
    if (!updated) {
      return c.json({ error: { code: "not_found", message: `Account ${id} not found` } }, 404);
    }
    return c.json(updated, 200);
  });

  app.openapi(deleteAccount, async (c) => {
    const { id } = c.req.valid("param");
    if (!deps.db)
      return c.json({ error: { code: "unavailable", message: "Store not configured" } }, 503);
    const uid = getUserId(c) ?? "";
    const removed = await deleteGhAccount(deps.db, uid, id);
    if (!removed) {
      return c.json({ error: { code: "not_found", message: `Account ${id} not found` } }, 404);
    }
    return c.body(null, 204);
  });
}
