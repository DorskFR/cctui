import { z } from "@hono/zod-openapi";

export const AccountSchema = z
  .string()
  .min(1)
  .openapi({ example: "DorskFR", description: "GitHub account/login the record was synced for" });

export const EtagSchema = z
  .string()
  .nullable()
  .openapi({ example: 'W/"a1b2c3"', description: "GitHub ETag captured at sync time, if any" });

export const SyncedAtSchema = z.string().datetime().openapi({
  example: "2026-07-12T09:00:00Z",
  description: "When the server last synced this record",
});

export const PayloadSchema = z.unknown().openapi({
  type: "object",
  description: "GitHub-shaped JSONB payload, relayed verbatim (narrow with octokit types)",
});

function envelope<K extends string>(kind: K, name: string) {
  return z
    .object({
      account: AccountSchema,
      kind: z.literal(kind).openapi({ example: kind }),
      synced_at: SyncedAtSchema,
      etag: EtagSchema,
      payload: PayloadSchema,
    })
    .openapi(name);
}

export const RepoEnvelopeSchema = envelope("repo", "RepoEnvelope");
export const PullRequestEnvelopeSchema = envelope("pull_request", "PullRequestEnvelope");
export const NotificationEnvelopeSchema = envelope("notification", "NotificationEnvelope");

export const PaginationQuerySchema = z.object({
  account: AccountSchema.optional().openapi({
    param: { name: "account", in: "query" },
    description: "Filter to a single account; omit to span all synced accounts",
  }),
  limit: z.coerce
    .number()
    .int()
    .min(1)
    .max(100)
    .default(30)
    .openapi({ param: { name: "limit", in: "query" }, example: 30 }),
  cursor: z
    .string()
    .optional()
    .openapi({
      param: { name: "cursor", in: "query" },
      description: "Opaque cursor from a previous page's next_cursor",
    }),
});

function page<T extends z.ZodTypeAny>(item: T, name: string) {
  return z
    .object({
      items: z.array(item),
      next_cursor: z
        .string()
        .nullable()
        .openapi({ description: "Cursor for the next page, or null at the end", example: null }),
    })
    .openapi(name);
}

export const RepoPageSchema = page(RepoEnvelopeSchema, "RepoPage");
export const PullRequestPageSchema = page(PullRequestEnvelopeSchema, "PullRequestPage");
export const NotificationPageSchema = page(NotificationEnvelopeSchema, "NotificationPage");

export const ErrorSchema = z
  .object({
    error: z.object({
      code: z
        .string()
        .openapi({ example: "not_found", description: "Stable machine-readable error code" }),
      message: z.string().openapi({ example: "Pull request not found" }),
      details: z
        .unknown()
        .optional()
        .openapi({ type: "object", description: "Optional structured context" }),
    }),
  })
  .openapi("Error");

export const StatusSchema = z
  .object({
    service: z.literal("gh-review"),
    version: z.string().openapi({ example: "0.0.1" }),
    api: z.literal("v1"),
    ok: z.boolean(),
    sync: z
      .object({
        last_run: z.string().datetime().nullable(),
        accounts: z.array(AccountSchema),
      })
      .openapi("SyncStatus"),
  })
  .openapi("Status");
