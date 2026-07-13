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

export const ReviewSideSchema = z
  .enum(["LEFT", "RIGHT"])
  .openapi({ example: "RIGHT", description: "Diff side: LEFT (old) or RIGHT (new)" });

export const ReviewVerdictSchema = z
  .enum(["comment", "approve", "request_changes"])
  .openapi({ example: "comment", description: "Review event when published to GitHub" });

export const ReviewDraftCommentSchema = z
  .object({
    id: z.string(),
    path: z.string().openapi({ example: "src/app.ts" }),
    side: ReviewSideSchema,
    line: z.number().int().openapi({ example: 42, description: "Line in the diff (GitHub line)" }),
    start_line: z.number().int().nullable().openapi({ description: "Start of a multi-line range" }),
    start_side: ReviewSideSchema.nullable(),
    body: z.string(),
    created_at: z.string().nullable(),
    updated_at: z.string().nullable(),
  })
  .openapi("ReviewDraftComment");

export const ReviewDraftSchema = z
  .object({
    id: z.string(),
    account: AccountSchema,
    owner: z.string(),
    repo: z.string(),
    pr_number: z.number().int(),
    head_sha: z.string().nullable().openapi({ description: "PR head captured when opened" }),
    verdict: ReviewVerdictSchema,
    body: z.string(),
    created_at: z.string().nullable(),
    updated_at: z.string().nullable(),
    comments: z.array(ReviewDraftCommentSchema),
  })
  .openapi("ReviewDraft");

export const ReviewDraftResultSchema = z
  .object({ draft: ReviewDraftSchema.nullable() })
  .openapi("ReviewDraftResult");

export const ReviewDraftCommentCreateSchema = z
  .object({
    account: AccountSchema,
    path: z.string().min(1),
    side: ReviewSideSchema.default("RIGHT"),
    line: z.number().int().positive(),
    start_line: z.number().int().positive().nullable().optional(),
    start_side: ReviewSideSchema.nullable().optional(),
    body: z.string().min(1),
    head_sha: z.string().optional().openapi({ description: "PR head to pin a new draft to" }),
  })
  .openapi("ReviewDraftCommentCreate");

export const ReviewDraftCommentEditSchema = z
  .object({
    account: AccountSchema,
    body: z.string().min(1).optional(),
    line: z.number().int().positive().optional(),
    side: ReviewSideSchema.optional(),
    start_line: z.number().int().positive().nullable().optional(),
    start_side: ReviewSideSchema.nullable().optional(),
  })
  .openapi("ReviewDraftCommentEdit");

export const ReviewDraftMetaSchema = z
  .object({
    account: AccountSchema,
    verdict: ReviewVerdictSchema.optional(),
    body: z.string().optional(),
  })
  .openapi("ReviewDraftMeta");

export const ReviewPublishSchema = z
  .object({
    account: AccountSchema,
    verdict: ReviewVerdictSchema,
    body: z.string().default(""),
  })
  .openapi("ReviewPublish");

export const SkippedReviewCommentSchema = z
  .object({
    path: z.string(),
    line: z.number().int(),
    reason: z.string().openapi({ example: "path not in pull request diff" }),
  })
  .openapi("SkippedReviewComment");

export const ReviewPublishResultSchema = z
  .object({
    published: z.boolean(),
    review_id: z.number().int().nullable(),
    posted: z.number().int().openapi({ description: "Comments accepted into the review" }),
    skipped: z.array(SkippedReviewCommentSchema),
  })
  .openapi("ReviewPublishResult");

export const ReactionRollupSchema = z
  .object({
    "+1": z.number().int(),
    "-1": z.number().int(),
    laugh: z.number().int(),
    hooray: z.number().int(),
    confused: z.number().int(),
    heart: z.number().int(),
    rocket: z.number().int(),
    eyes: z.number().int(),
    total_count: z.number().int(),
  })
  .partial()
  .openapi("ReactionRollup");

export const ReviewThreadCommentSchema = z
  .object({
    id: z.number().int(),
    path: z.string().nullable(),
    line: z.number().int().nullable(),
    original_line: z.number().int().nullable(),
    side: z.string().nullable(),
    start_line: z.number().int().nullable(),
    diff_hunk: z.string().nullable(),
    body: z.string(),
    user: z.string().nullable(),
    in_reply_to_id: z.number().int().nullable(),
    created_at: z.string().nullable(),
    html_url: z.string().nullable(),
    reactions: ReactionRollupSchema.nullable(),
  })
  .openapi("ReviewThreadComment");

export const ReviewThreadListSchema = z
  .object({ items: z.array(ReviewThreadCommentSchema) })
  .openapi("ReviewThreadList");

export const ReactionContentSchema = z
  .enum(["+1", "-1", "laugh", "confused", "heart", "hooray", "rocket", "eyes"])
  .openapi({ example: "+1", description: "GitHub reaction content" });

export const ReactionToggleSchema = z
  .object({
    account: AccountSchema,
    content: ReactionContentSchema,
  })
  .openapi("ReactionToggle");

export const ReactionSummarySchema = z
  .object({
    "+1": z.number().int(),
    "-1": z.number().int(),
    laugh: z.number().int(),
    hooray: z.number().int(),
    confused: z.number().int(),
    heart: z.number().int(),
    rocket: z.number().int(),
    eyes: z.number().int(),
    total_count: z.number().int(),
    viewer_reactions: z
      .array(ReactionContentSchema)
      .openapi({ description: "Reaction contents the calling account currently holds" }),
  })
  .openapi("ReactionSummary");

export const LabelSchema = z
  .object({
    name: z.string().openapi({ example: "bug" }),
    color: z.string().openapi({ example: "d73a4a", description: "GitHub hex color, no leading #" }),
    description: z.string().nullable().openapi({ example: "Something isn't working" }),
  })
  .openapi("Label");

export const RepoLabelListSchema = z
  .object({ items: z.array(LabelSchema) })
  .openapi("RepoLabelList");

export const LabelMutateSchema = z
  .object({
    account: AccountSchema,
    name: z.string().min(1).openapi({ example: "bug", description: "Label name to add" }),
  })
  .openapi("LabelMutate");

export const PullLabelsSchema = z.object({ labels: z.array(LabelSchema) }).openapi("PullLabels");

export const CommentDeleteResultSchema = z
  .object({ deleted: z.boolean().openapi({ example: true }) })
  .openapi("CommentDeleteResult");

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

export const SyncRequestSchema = z
  .object({
    account: AccountSchema.optional().openapi({
      description:
        "The caller's GitHub login to force-sync; omit when the caller has exactly one account",
    }),
  })
  .openapi("SyncRequest");

export const SyncResultSchema = z
  .object({
    account: AccountSchema,
    status: z.literal("ok").openapi({
      description: "The incremental poll completed",
    }),
  })
  .openapi("SyncResult");

export const MergeMethodSchema = z
  .enum(["merge", "squash", "rebase"])
  .openapi({ example: "squash", description: "How GitHub combines the commits when merging" });

export const MergePullSchema = z
  .object({
    account: AccountSchema,
    merge_method: MergeMethodSchema.default("squash"),
    expected_head_sha: z
      .string()
      .optional()
      .openapi({ description: "Guard: reject when the PR head moved from this SHA" }),
  })
  .openapi("MergePull");

export const MergeResultSchema = z
  .object({
    merged: z.boolean().openapi({ example: true }),
    sha: z.string().nullable().openapi({ description: "The resulting merge commit SHA" }),
    message: z.string().nullable().openapi({ example: "Pull Request successfully merged" }),
  })
  .openapi("MergeResult");

export const ReviewerStateSchema = z
  .enum(["APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED", "PENDING"])
  .openapi({ example: "APPROVED", description: "Latest effective review state for a reviewer" });

export const ReviewerSchema = z
  .object({
    login: z.string().openapi({ example: "octocat" }),
    avatar_url: z.string().nullable(),
    state: ReviewerStateSchema,
    requested: z
      .boolean()
      .openapi({ description: "Whether a review is currently requested from this reviewer" }),
  })
  .openapi("Reviewer");

export const RequestedTeamSchema = z
  .object({
    name: z.string().openapi({ example: "Platform" }),
    slug: z.string().openapi({ example: "platform" }),
  })
  .openapi("RequestedTeam");

export const ReviewersResultSchema = z
  .object({
    reviewers: z.array(ReviewerSchema),
    requested_teams: z.array(RequestedTeamSchema),
  })
  .openapi("ReviewersResult");

export const ReRequestReviewersSchema = z
  .object({
    account: AccountSchema,
    reviewers: z.array(z.string().min(1)).min(1),
  })
  .openapi("ReRequestReviewers");

export const ActivityActorSchema = z
  .object({
    login: z.string().openapi({ example: "octocat" }),
    avatar_url: z.string().nullable(),
  })
  .openapi("ActivityActor");

export const ActivityDetailSchema = z
  .object({
    sha: z.string().optional().openapi({ description: "Short commit SHA for commit/merge/close" }),
    message: z.string().optional().openapi({ description: "Commit subject line" }),
    author_name: z.string().optional().openapi({ description: "Git author of a commit" }),
    state: z
      .string()
      .optional()
      .openapi({ example: "APPROVED", description: "Review state, uppercased" }),
    body: z.string().optional().openapi({ description: "Review/comment body excerpt" }),
    label: z
      .object({ name: z.string(), color: z.string().nullable() })
      .optional()
      .openapi({ description: "Label added or removed" }),
    reviewer: ActivityActorSchema.optional().openapi({ description: "Reviewer requested" }),
    team: z.string().optional().openapi({ description: "Team review requested from" }),
    assignee: ActivityActorSchema.optional().openapi({ description: "Assignee added or removed" }),
    from: z.string().optional().openapi({ description: "Previous title on a rename" }),
    to: z.string().optional().openapi({ description: "New title on a rename" }),
  })
  .openapi("ActivityDetail");

export const ActivityEventSchema = z
  .object({
    event: z.string().openapi({ example: "reviewed", description: "GitHub timeline event type" }),
    actor: ActivityActorSchema.nullable(),
    created_at: z.string().nullable().openapi({ description: "ISO timestamp of the event" }),
    detail: ActivityDetailSchema.optional(),
  })
  .openapi("ActivityEvent");

export const ActivityListSchema = z
  .object({ items: z.array(ActivityEventSchema) })
  .openapi("ActivityList");
