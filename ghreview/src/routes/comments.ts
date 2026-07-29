import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { requireOwnedAccount } from "../auth/ownership.ts";
import type { AppDeps } from "../deps.ts";
import { deleteIssueComment, deletePullReviewComment } from "../github/comments.ts";
import { AccountSchema, CommentDeleteResultSchema, ErrorSchema } from "../schemas.ts";

const CommentParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
  commentId: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "commentId", in: "path" }, example: 1 }),
});

const AccountQuery = z.object({
  account: AccountSchema.openapi({ param: { name: "account", in: "query" } }),
});

const responses = {
  200: {
    description: "The comment was deleted",
    content: { "application/json": { schema: CommentDeleteResultSchema } },
  },
  403: {
    description: "Caller does not own the account, or GitHub forbids the delete",
    content: { "application/json": { schema: ErrorSchema } },
  },
  404: {
    description: "Account not managed, or the comment does not exist",
    content: { "application/json": { schema: ErrorSchema } },
  },
} as const;

const deleteReviewCommentRoute = createRoute({
  method: "delete",
  path: "/v1/repos/{owner}/{repo}/pulls/comments/{commentId}",
  summary: "Delete a published review (per-line) comment",
  tags: ["comments"],
  request: { params: CommentParams, query: AccountQuery },
  responses,
});

const deleteIssueCommentRoute = createRoute({
  method: "delete",
  path: "/v1/repos/{owner}/{repo}/issues/comments/{commentId}",
  summary: "Delete a published issue/conversation comment",
  tags: ["comments"],
  request: { params: CommentParams, query: AccountQuery },
  responses,
});

interface GithubErrorShape {
  status?: number;
  message?: string;
  response?: { data?: { message?: string } };
}

function mapGithubError(err: unknown): { code: string; message: string; status: 403 | 404 } | null {
  const e = err as GithubErrorShape;
  if (e.status !== 403 && e.status !== 404) return null;
  const message = e.response?.data?.message ?? e.message ?? "GitHub rejected the request";
  return { code: e.status === 403 ? "forbidden" : "not_found", message, status: e.status };
}

function notifyPull(deps: AppDeps, account: string, owner: string, repo: string, number: number) {
  deps.bus?.publishNotice({ account, kind: "pull_request", key: `${owner}/${repo}#${number}` });
}

export function registerComments(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(deleteReviewCommentRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const auth = await requireOwnedAccount(deps, c, account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    try {
      const number = await deletePullReviewComment(auth.acct.octokit, p.owner, p.repo, p.commentId);
      if (number !== null) notifyPull(deps, account, p.owner, p.repo, number);
      return c.json({ deleted: true }, 200);
    } catch (err) {
      const mapped = mapGithubError(err);
      if (!mapped) throw err;
      return c.json({ error: { code: mapped.code, message: mapped.message } }, mapped.status);
    }
  });

  app.openapi(deleteIssueCommentRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const auth = await requireOwnedAccount(deps, c, account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    try {
      const number = await deleteIssueComment(auth.acct.octokit, p.owner, p.repo, p.commentId);
      if (number !== null) notifyPull(deps, account, p.owner, p.repo, number);
      return c.json({ deleted: true }, 200);
    } catch (err) {
      const mapped = mapGithubError(err);
      if (!mapped) throw err;
      return c.json({ error: { code: mapped.code, message: mapped.message } }, mapped.status);
    }
  });
}
