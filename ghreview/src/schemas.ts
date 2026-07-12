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

export const NotificationStateSchema = z
  .object({
    read: z.boolean().openapi({ description: "Locally marked read (also pushed to GitHub)" }),
    done: z.boolean().openapi({ description: "Locally marked done (local-only state)" }),
    archived: z.boolean().openapi({ description: "Locally archived (local-only state)" }),
    read_at: z.string().nullable(),
    done_at: z.string().nullable(),
    archived_at: z.string().nullable(),
    push_pending: z
      .boolean()
      .openapi({ description: "A mark-as-read still owed to GitHub; retried on the next poll" }),
    last_error: z.string().nullable().openapi({ description: "Last push failure, if any" }),
    updated_at: z.string().nullable(),
  })
  .openapi("NotificationState");

export const NotificationInboxItemSchema = z
  .object({
    account: AccountSchema,
    kind: z.literal("notification").openapi({ example: "notification" }),
    synced_at: SyncedAtSchema,
    etag: EtagSchema,
    payload: PayloadSchema,
    state: NotificationStateSchema,
  })
  .openapi("NotificationInboxItem");

export const NotificationInboxPageSchema = page(
  NotificationInboxItemSchema,
  "NotificationInboxPage",
);

export const NotificationStateItemSchema = z
  .object({ thread_id: z.string(), state: NotificationStateSchema })
  .openapi("NotificationStateItem");

export const NotificationStateResultSchema = z
  .object({ items: z.array(NotificationStateItemSchema) })
  .openapi("NotificationStateResult");

const StatePatchShape = {
  read: z.boolean().optional().openapi({ description: "Set read; true pushes to GitHub" }),
  done: z.boolean().optional().openapi({ description: "Set done (local-only)" }),
  archived: z.boolean().optional().openapi({ description: "Set archived (local-only)" }),
};

const atLeastOneFlag = (v: { read?: boolean; done?: boolean; archived?: boolean }) =>
  v.read !== undefined || v.done !== undefined || v.archived !== undefined;

export const NotificationSingleStateSchema = z
  .object({ account: AccountSchema, ...StatePatchShape })
  .refine(atLeastOneFlag, { message: "Provide at least one of read, done, archived" })
  .openapi("NotificationSingleState");

export const NotificationBulkStateSchema = z
  .object({
    account: AccountSchema,
    thread_ids: z
      .array(z.string().min(1))
      .min(1)
      .max(200)
      .openapi({ description: "Notification thread ids to mutate" }),
    ...StatePatchShape,
  })
  .refine(atLeastOneFlag, { message: "Provide at least one of read, done, archived" })
  .openapi("NotificationBulkState");

const boolParam = (name: string, description: string) =>
  z
    .enum(["true", "false"])
    .optional()
    .openapi({ param: { name, in: "query" }, description });

export const NotificationInboxQuerySchema = PaginationQuerySchema.extend({
  limit: z.coerce
    .number()
    .int()
    .min(1)
    .max(5000)
    .default(30)
    .openapi({
      param: { name: "limit", in: "query" },
      example: 30,
      description: "Page size (up to 5000). Ignored when all=true.",
    }),
  all: z
    .enum(["true", "false"])
    .optional()
    .openapi({
      param: { name: "all", in: "query" },
      description:
        "true: return the entire notification set in one response (ignores limit/cursor)",
    }),
  reason: z
    .string()
    .optional()
    .openapi({
      param: { name: "reason", in: "query" },
      description:
        "GitHub reason (review_requested/mention/ci_activity; aliases review-requested/ci)",
    }),
  repo: z
    .string()
    .optional()
    .openapi({
      param: { name: "repo", in: "query" },
      example: "DorskFR/cctui",
      description: "Filter by repository full name",
    }),
  unread: boolParam("unread", "true: only unread & not locally read; false: read"),
  undone: boolParam("undone", "true: only not-done; false: only done"),
  archived: boolParam("archived", "true: only archived; default/false: hide archived"),
  since: z
    .string()
    .datetime()
    .optional()
    .openapi({
      param: { name: "since", in: "query" },
      description: "Only notifications updated at/after this ISO timestamp",
    }),
});

export const ViewedStateItemSchema = z
  .object({
    path: z.string().openapi({ description: "File path within the pull request" }),
    viewed: z.boolean().openapi({ description: "Marked viewed (mirrored to github.com)" }),
    digest: z
      .string()
      .nullable()
      .openapi({ description: "File blob sha / patch digest recorded at mark time" }),
    push_pending: z.boolean().openapi({
      description: "A viewed change still owed to github.com; retried on the next poll",
    }),
    last_error: z.string().nullable().openapi({ description: "Last push failure, if any" }),
    updated_at: z.string().nullable(),
  })
  .openapi("ViewedStateItem");

export const ViewedStateResultSchema = z
  .object({ items: z.array(ViewedStateItemSchema) })
  .openapi("ViewedStateResult");

export const ViewedStateSetSchema = z
  .object({
    account: AccountSchema,
    paths: z
      .array(z.string().min(1))
      .min(1)
      .max(1000)
      .openapi({ description: "File paths to mark; a folder op sends every file beneath it" }),
    viewed: z.boolean().openapi({ description: "Target viewed state for all paths" }),
  })
  .openapi("ViewedStateSet");

export const AccountCreateSchema = z
  .object({
    token: z.string().min(1).openapi({
      description: "GitHub PAT (fine-grained preferred); validated, sealed, never returned",
    }),
    login: z
      .string()
      .min(1)
      .optional()
      .openapi({ description: "Expected login; rejected if it does not match the PAT's account" }),
    poll_interval_ms: z.number().int().min(1000).optional(),
    budget_ceiling: z.number().min(0).max(1).optional(),
    rate_limit: z.number().int().min(1).optional(),
  })
  .openapi("AccountCreate");

export const AccountSummarySchema = z
  .object({
    id: z.string(),
    login: AccountSchema,
    poll_interval_ms: z.number().int().nullable(),
    budget_ceiling: z.number().nullable(),
    rate_limit: z.number().int().nullable(),
    active: z.boolean(),
    created_at: z.string().nullable(),
  })
  .openapi("AccountSummary");

export const AccountListSchema = z
  .object({ items: z.array(AccountSummarySchema) })
  .openapi("AccountList");

export const SubscriptionKindSchema = z
  .enum(["repo", "pull_request", "notification"])
  .openapi({ example: "pull_request", description: "Subscription kind" });

export const SubscriptionCreateSchema = z
  .object({
    kind: SubscriptionKindSchema.default("pull_request"),
    target: z.string().min(1).openapi({
      description:
        "For pull_request: a github.com PR URL or `owner/repo#number`. For repo: `owner/repo`.",
      example: "https://github.com/DorskFR/cctui/pull/42",
    }),
    account: AccountSchema.optional().openapi({
      description:
        "The caller's GitHub login to own the subscription; omit when the caller has exactly one account",
    }),
  })
  .openapi("SubscriptionCreate");

export const SubscriptionSchema = z
  .object({
    id: z.string(),
    account: AccountSchema,
    kind: SubscriptionKindSchema,
    target: z.string().nullable(),
    active: z.boolean(),
    created_at: z.string().nullable(),
  })
  .openapi("Subscription");

export const SubscriptionListSchema = z
  .object({ items: z.array(SubscriptionSchema) })
  .openapi("SubscriptionList");

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
