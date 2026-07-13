import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { getDocument, upsertDocument } from "../db/documents.ts";
import { accountOwnedBy } from "../db/notificationState.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import { type ReactionSummary, toggleReaction } from "../github/reactions.ts";
import { ErrorSchema, ReactionSummarySchema, ReactionToggleSchema } from "../schemas.ts";

const RepoParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
});

const PrParams = RepoParams.extend({
  number: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "number", in: "path" }, example: 42 }),
});

const CommentParams = RepoParams.extend({
  commentId: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "commentId", in: "path" }, example: 1 }),
});

const responses = {
  200: {
    description: "Updated reaction summary",
    content: { "application/json": { schema: ReactionSummarySchema } },
  },
  403: {
    description: "Caller does not own the account",
    content: { "application/json": { schema: ErrorSchema } },
  },
  404: {
    description: "Account not managed",
    content: { "application/json": { schema: ErrorSchema } },
  },
} as const;

const togglePullRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/reactions",
  summary: "Toggle the caller's reaction on a pull request description",
  tags: ["reactions"],
  request: {
    params: PrParams,
    body: { content: { "application/json": { schema: ReactionToggleSchema } } },
  },
  responses,
});

const toggleIssueCommentRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/issues/comments/{commentId}/reactions",
  summary: "Toggle the caller's reaction on an issue/conversation comment",
  tags: ["reactions"],
  request: {
    params: CommentParams,
    body: { content: { "application/json": { schema: ReactionToggleSchema } } },
  },
  responses,
});

const toggleReviewCommentRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/comments/{commentId}/reactions",
  summary: "Toggle the caller's reaction on a review (per-line) comment",
  tags: ["reactions"],
  request: {
    params: CommentParams,
    body: { content: { "application/json": { schema: ReactionToggleSchema } } },
  },
  responses,
});

async function patchPullReactions(
  deps: AppDeps,
  account: string,
  owner: string,
  repo: string,
  number: number,
  summary: ReactionSummary,
): Promise<void> {
  if (!deps.db) return;
  const key = `${owner}/${repo}#${number}`;
  const doc = await getDocument(deps.db, account, "pull_request", key);
  if (!doc) return;
  const payload = { ...(doc.payload as Record<string, unknown>) };
  const prev = (payload.reactions as Record<string, unknown> | undefined) ?? {};
  payload.reactions = {
    ...prev,
    "+1": summary["+1"],
    "-1": summary["-1"],
    laugh: summary.laugh,
    hooray: summary.hooray,
    confused: summary.confused,
    heart: summary.heart,
    rocket: summary.rocket,
    eyes: summary.eyes,
    total_count: summary.total_count,
  };
  await upsertDocument(deps.db, {
    account,
    kind: "pull_request",
    key,
    etag: doc.etag,
    payload,
  });
}

type AuthResult =
  | { ok: true; acct: Account }
  | { ok: false; code: "forbidden" | "not_found"; message: string; status: 403 | 404 };

async function authAccount(
  deps: AppDeps,
  uid: string | undefined,
  account: string,
): Promise<AuthResult> {
  if (deps.db && uid !== undefined && !(await accountOwnedBy(deps.db, account, uid))) {
    return {
      ok: false,
      code: "forbidden",
      message: `Account ${account} is not accessible`,
      status: 403,
    };
  }
  const acct = deps.accountFor?.(account);
  if (!acct) {
    return {
      ok: false,
      code: "not_found",
      message: `Account ${account} is not managed`,
      status: 404,
    };
  }
  return { ok: true, acct };
}

export function registerReactions(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(togglePullRoute, async (c) => {
    const p = c.req.valid("param");
    const { account, content } = c.req.valid("json");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    const base = `/repos/${p.owner}/${p.repo}/issues/${p.number}`;
    const summary = await toggleReaction(auth.acct.octokit, base, auth.acct.login, content);
    await patchPullReactions(deps, account, p.owner, p.repo, p.number, summary);
    return c.json(summary, 200);
  });

  app.openapi(toggleIssueCommentRoute, async (c) => {
    const p = c.req.valid("param");
    const { account, content } = c.req.valid("json");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    const base = `/repos/${p.owner}/${p.repo}/issues/comments/${p.commentId}`;
    const summary = await toggleReaction(auth.acct.octokit, base, auth.acct.login, content);
    return c.json(summary, 200);
  });

  app.openapi(toggleReviewCommentRoute, async (c) => {
    const p = c.req.valid("param");
    const { account, content } = c.req.valid("json");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    const base = `/repos/${p.owner}/${p.repo}/pulls/comments/${p.commentId}`;
    const summary = await toggleReaction(auth.acct.octokit, base, auth.acct.login, content);
    return c.json(summary, 200);
  });
}
