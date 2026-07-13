import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { getDocument } from "../db/documents.ts";
import { accountOwnedBy } from "../db/notificationState.ts";
import {
  addDraftComment,
  clearDraft,
  deleteDraftComment,
  editDraftComment,
  getDraft,
  openDraft,
  type PullRef,
  type ReviewVerdict,
  updateDraftMeta,
} from "../db/reviewDrafts.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import {
  ErrorSchema,
  ReviewDraftCommentCreateSchema,
  ReviewDraftCommentEditSchema,
  ReviewDraftMetaSchema,
  ReviewDraftResultSchema,
  ReviewPublishResultSchema,
  ReviewPublishSchema,
  ReviewThreadListSchema,
} from "../schemas.ts";

const PullParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
  number: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "number", in: "path" }, example: 42 }),
});

const CommentParams = PullParams.extend({
  commentId: z.string().openapi({ param: { name: "commentId", in: "path" }, example: "1" }),
});

const AccountQuery = z.object({
  account: z
    .string()
    .min(1)
    .openapi({ param: { name: "account", in: "query" }, example: "DorskFR" }),
});

const EVENT: Record<ReviewVerdict, string> = {
  comment: "COMMENT",
  approve: "APPROVE",
  request_changes: "REQUEST_CHANGES",
};

function refOf(p: { owner: string; repo: string; number: number }): PullRef {
  return { owner: p.owner, repo: p.repo, number: p.number };
}

function pullKey(ref: PullRef): string {
  return `${ref.owner}/${ref.repo}#${ref.number}`;
}

async function storedHeadSha(deps: AppDeps, account: string, ref: PullRef): Promise<string | null> {
  if (!deps.db) return null;
  const doc = await getDocument(deps.db, account, "pull_request", pullKey(ref));
  const payload = doc?.payload as { head?: { sha?: string } } | undefined;
  return payload?.head?.sha ?? null;
}

async function fetchLiveHeadSha(octokit: Account["octokit"], ref: PullRef): Promise<string | null> {
  const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}", {
    owner: ref.owner,
    repo: ref.repo,
    pull_number: ref.number,
  });
  const data = res.data as { head?: { sha?: string } };
  return data?.head?.sha ?? null;
}

async function fetchPullPaths(octokit: Account["octokit"], ref: PullRef): Promise<Set<string>> {
  const paths = new Set<string>();
  for (let page = 1; page <= 20; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/files", {
      owner: ref.owner,
      repo: ref.repo,
      pull_number: ref.number,
      per_page: 100,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as { filename?: string }[]) : [];
    for (const f of batch) if (f.filename) paths.add(f.filename);
    if (batch.length < 100) break;
  }
  return paths;
}

const getDraftRoute = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/review-draft",
  summary: "Get the caller's review draft for a pull request",
  tags: ["reviews"],
  request: { params: PullParams, query: AccountQuery },
  responses: {
    200: {
      description: "The review draft, or null when none exists",
      content: { "application/json": { schema: ReviewDraftResultSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const patchDraftRoute = createRoute({
  method: "patch",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/review-draft",
  summary: "Update the draft verdict/summary body",
  tags: ["reviews"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: ReviewDraftMetaSchema } } },
  },
  responses: {
    200: {
      description: "The updated draft",
      content: { "application/json": { schema: ReviewDraftResultSchema } },
    },
    404: {
      description: "No draft / account not owned",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const addCommentRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/review-draft/comments",
  summary: "Add a per-line comment to the draft (opens one if needed)",
  tags: ["reviews"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: ReviewDraftCommentCreateSchema } } },
  },
  responses: {
    201: {
      description: "The updated draft",
      content: { "application/json": { schema: ReviewDraftResultSchema } },
    },
    404: {
      description: "Account not owned by the caller",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const editCommentRoute = createRoute({
  method: "patch",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/review-draft/comments/{commentId}",
  summary: "Edit a draft comment",
  tags: ["reviews"],
  request: {
    params: CommentParams,
    body: { content: { "application/json": { schema: ReviewDraftCommentEditSchema } } },
  },
  responses: {
    200: {
      description: "The updated draft",
      content: { "application/json": { schema: ReviewDraftResultSchema } },
    },
    404: {
      description: "Comment not found",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const deleteCommentRoute = createRoute({
  method: "delete",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/review-draft/comments/{commentId}",
  summary: "Delete a draft comment",
  tags: ["reviews"],
  request: { params: CommentParams, query: AccountQuery },
  responses: {
    200: {
      description: "The updated draft",
      content: { "application/json": { schema: ReviewDraftResultSchema } },
    },
    404: {
      description: "Comment not found",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const publishRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/review-draft/publish",
  summary: "Publish the draft as one batched GitHub review",
  tags: ["reviews"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: ReviewPublishSchema } } },
  },
  responses: {
    200: {
      description: "Publish result",
      content: { "application/json": { schema: ReviewPublishResultSchema } },
    },
    404: {
      description: "No draft / account not owned",
      content: { "application/json": { schema: ErrorSchema } },
    },
    409: {
      description: "PR head moved since the draft was opened",
      content: { "application/json": { schema: ErrorSchema } },
    },
    422: {
      description: "Nothing to publish",
      content: { "application/json": { schema: ErrorSchema } },
    },
    503: {
      description: "Store unavailable",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const listThreadsRoute = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/comments",
  summary: "List existing published review comments for a pull request",
  tags: ["reviews"],
  request: { params: PullParams, query: AccountQuery },
  responses: {
    200: {
      description: "Published review comments",
      content: { "application/json": { schema: ReviewThreadListSchema } },
    },
    403: {
      description: "Caller does not own the account",
      content: { "application/json": { schema: ErrorSchema } },
    },
    404: {
      description: "Account not managed",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const STORE_ERR = { error: { code: "unavailable", message: "Store not configured" } } as const;

export function registerReviews(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(getDraftRoute, async (c) => {
    if (!deps.db) return c.json(STORE_ERR, 503);
    const uid = getUserId(c) ?? "";
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const draft = await getDraft(deps.db, uid, account, refOf(p));
    return c.json({ draft }, 200);
  });

  app.openapi(patchDraftRoute, async (c) => {
    if (!deps.db) return c.json(STORE_ERR, 503);
    const uid = getUserId(c) ?? "";
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    await openDraft(
      deps.db,
      uid,
      body.account,
      refOf(p),
      await storedHeadSha(deps, body.account, refOf(p)),
    );
    const draft = await updateDraftMeta(deps.db, uid, body.account, refOf(p), {
      verdict: body.verdict,
      body: body.body,
    });
    if (!draft) return c.json({ error: { code: "not_found", message: "No draft" } }, 404);
    return c.json({ draft }, 200);
  });

  app.openapi(addCommentRoute, async (c) => {
    if (!deps.db) return c.json(STORE_ERR, 503);
    const uid = getUserId(c) ?? "";
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const ref = refOf(p);
    const headSha = body.head_sha ?? (await storedHeadSha(deps, body.account, ref));
    const draft = await addDraftComment(deps.db, uid, body.account, ref, headSha, {
      path: body.path,
      side: body.side,
      line: body.line,
      start_line: body.start_line ?? null,
      start_side: body.start_side ?? null,
      body: body.body,
    });
    if (!draft) {
      return c.json(
        {
          error: {
            code: "not_found",
            message: `Account ${body.account} is not owned by the caller`,
          },
        },
        404,
      );
    }
    return c.json({ draft }, 201);
  });

  app.openapi(editCommentRoute, async (c) => {
    if (!deps.db) return c.json(STORE_ERR, 503);
    const uid = getUserId(c) ?? "";
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const draft = await editDraftComment(deps.db, uid, body.account, refOf(p), p.commentId, {
      body: body.body,
      line: body.line,
      side: body.side,
      start_line: body.start_line,
      start_side: body.start_side,
    });
    if (!draft) return c.json({ error: { code: "not_found", message: "Comment not found" } }, 404);
    return c.json({ draft }, 200);
  });

  app.openapi(deleteCommentRoute, async (c) => {
    if (!deps.db) return c.json(STORE_ERR, 503);
    const uid = getUserId(c) ?? "";
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const draft = await deleteDraftComment(deps.db, uid, account, refOf(p), p.commentId);
    if (!draft) return c.json({ error: { code: "not_found", message: "Comment not found" } }, 404);
    return c.json({ draft }, 200);
  });

  app.openapi(publishRoute, async (c) => {
    if (!deps.db) return c.json(STORE_ERR, 503);
    const uid = getUserId(c) ?? "";
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const ref = refOf(p);
    const draft = await getDraft(deps.db, uid, body.account, ref);
    if (!draft)
      return c.json({ error: { code: "not_found", message: "No draft to publish" } }, 404);

    const acct = deps.accountFor?.(body.account);
    if (!acct)
      return c.json(
        { error: { code: "not_found", message: `Account ${body.account} is not managed` } },
        404,
      );

    const liveHead = await fetchLiveHeadSha(acct.octokit, ref);
    if (draft.head_sha && liveHead && draft.head_sha !== liveHead) {
      return c.json(
        {
          error: {
            code: "stale_head",
            message: "The pull request head moved since this draft was opened",
            details: { draft_head: draft.head_sha, current_head: liveHead },
          },
        },
        409,
      );
    }

    const paths = await fetchPullPaths(acct.octokit, ref);
    const skipped: { path: string; line: number; reason: string }[] = [];
    const comments: Record<string, unknown>[] = [];
    for (const cm of draft.comments) {
      if (!paths.has(cm.path)) {
        skipped.push({ path: cm.path, line: cm.line, reason: "path not in pull request diff" });
        continue;
      }
      const entry: Record<string, unknown> = {
        path: cm.path,
        body: cm.body,
        line: cm.line,
        side: cm.side,
      };
      if (cm.start_line !== null) {
        entry.start_line = cm.start_line;
        entry.start_side = cm.start_side ?? cm.side;
      }
      comments.push(entry);
    }

    if (comments.length === 0 && body.verdict === "comment" && !body.body.trim()) {
      return c.json(
        { error: { code: "empty_review", message: "Nothing to publish", details: { skipped } } },
        422,
      );
    }

    const res = await acct.octokit.request(
      "POST /repos/{owner}/{repo}/pulls/{pull_number}/reviews",
      {
        owner: ref.owner,
        repo: ref.repo,
        pull_number: ref.number,
        event: EVENT[body.verdict],
        body: body.body,
        commit_id: liveHead ?? undefined,
        comments,
      },
    );
    const review = res.data as { id?: number };

    await clearDraft(deps.db, draft.id);
    return c.json(
      { published: true, review_id: review?.id ?? null, posted: comments.length, skipped },
      200,
    );
  });

  app.openapi(listThreadsRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const uid = getUserId(c);
    if (deps.db && uid !== undefined && !(await accountOwnedBy(deps.db, account, uid))) {
      return c.json(
        { error: { code: "forbidden", message: `Account ${account} is not accessible` } },
        403,
      );
    }
    const acct = deps.accountFor?.(account);
    if (!acct)
      return c.json(
        { error: { code: "not_found", message: `Account ${account} is not managed` } },
        404,
      );
    const ref = refOf(p);
    const items: {
      id: number;
      path: string | null;
      line: number | null;
      original_line: number | null;
      side: string | null;
      start_line: number | null;
      diff_hunk: string | null;
      body: string;
      user: string | null;
      in_reply_to_id: number | null;
      created_at: string | null;
      html_url: string | null;
      reactions: Record<string, number> | null;
    }[] = [];
    for (let page = 1; page <= 20; page++) {
      const res = await acct.octokit.request(
        "GET /repos/{owner}/{repo}/pulls/{pull_number}/comments",
        {
          owner: ref.owner,
          repo: ref.repo,
          pull_number: ref.number,
          per_page: 100,
          page,
        },
      );
      const batch = Array.isArray(res.data) ? (res.data as Record<string, unknown>[]) : [];
      for (const rc of batch) {
        items.push({
          id: Number(rc.id ?? 0),
          path: (rc.path as string | null | undefined) ?? null,
          line: (rc.line as number | null | undefined) ?? null,
          original_line: (rc.original_line as number | null | undefined) ?? null,
          side: (rc.side as string | null | undefined) ?? null,
          start_line: (rc.start_line as number | null | undefined) ?? null,
          diff_hunk: (rc.diff_hunk as string | null | undefined) ?? null,
          body: String(rc.body ?? ""),
          user: ((rc.user as { login?: string } | undefined)?.login as string | undefined) ?? null,
          in_reply_to_id: (rc.in_reply_to_id as number | null | undefined) ?? null,
          created_at: (rc.created_at as string | null | undefined) ?? null,
          html_url: (rc.html_url as string | null | undefined) ?? null,
          reactions: (rc.reactions as Record<string, number> | null | undefined) ?? null,
        });
      }
      if (batch.length < 100) break;
    }
    return c.json({ items }, 200);
  });
}
