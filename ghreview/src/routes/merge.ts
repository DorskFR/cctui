import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { requireOwnedAccount } from "../auth/ownership.ts";
import { deleteDocument } from "../db/documents.ts";
import { deleteReviewDraftsForPull } from "../db/reviewDrafts.ts";
import { deactivateSubscription } from "../db/subscriptions.ts";
import { deleteViewedStateForPull } from "../db/viewedState.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import { ErrorSchema, MergePullSchema, MergeResultSchema } from "../schemas.ts";

const PullParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
  number: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "number", in: "path" }, example: 42 }),
});

const mergeRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/merge",
  summary: "Merge a pull request and drop it from the store",
  tags: ["pulls"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: MergePullSchema } } },
  },
  responses: {
    200: {
      description: "Merge result",
      content: { "application/json": { schema: MergeResultSchema } },
    },
    403: {
      description: "Caller does not own the account",
      content: { "application/json": { schema: ErrorSchema } },
    },
    404: {
      description: "Account not managed",
      content: { "application/json": { schema: ErrorSchema } },
    },
    405: {
      description: "Pull request is not mergeable",
      content: { "application/json": { schema: ErrorSchema } },
    },
    409: {
      description: "PR head moved since the expected SHA",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

async function fetchLiveHeadSha(
  octokit: Account["octokit"],
  p: {
    owner: string;
    repo: string;
    number: number;
  },
): Promise<string | null> {
  const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}", {
    owner: p.owner,
    repo: p.repo,
    pull_number: p.number,
  });
  const data = res.data as { head?: { sha?: string } };
  return data?.head?.sha ?? null;
}

async function removePull(
  deps: AppDeps,
  account: string,
  owner: string,
  repo: string,
  number: number,
): Promise<void> {
  if (!deps.db) return;
  const ref = { owner, repo, number };
  const key = `${owner}/${repo}#${number}`;
  await deleteDocument(deps.db, account, "pull_request", key);
  await deleteViewedStateForPull(deps.db, account, ref);
  await deleteReviewDraftsForPull(deps.db, account, ref);
  await deactivateSubscription(deps.db, account, "pull_request", key);
}

interface GithubErrorShape {
  status?: number;
  message?: string;
  response?: { data?: { message?: string } };
}

function mapGithubError(err: unknown): { code: string; message: string; status: 405 | 409 } | null {
  const e = err as GithubErrorShape;
  if (e.status !== 405 && e.status !== 409) return null;
  const message = e.response?.data?.message ?? e.message ?? "GitHub rejected the merge";
  return {
    code: e.status === 405 ? "not_mergeable" : "head_mismatch",
    message,
    status: e.status,
  };
}

export function registerMerge(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(mergeRoute, async (c) => {
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const auth = await requireOwnedAccount(deps, c, body.account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    const acct = auth.acct;

    if (body.expected_head_sha) {
      const liveHead = await fetchLiveHeadSha(acct.octokit, p);
      if (liveHead && liveHead !== body.expected_head_sha) {
        return c.json(
          {
            error: {
              code: "stale_head",
              message: "The pull request head moved since it was loaded",
              details: { expected_head: body.expected_head_sha, current_head: liveHead },
            },
          },
          409,
        );
      }
    }

    try {
      const res = await acct.octokit.request(
        "PUT /repos/{owner}/{repo}/pulls/{pull_number}/merge",
        {
          owner: p.owner,
          repo: p.repo,
          pull_number: p.number,
          merge_method: body.merge_method,
          sha: body.expected_head_sha,
        },
      );
      const data = res.data as { merged?: boolean; sha?: string; message?: string };
      if (data?.merged) {
        await removePull(deps, body.account, p.owner, p.repo, p.number);
      }
      return c.json(
        {
          merged: data?.merged ?? false,
          sha: data?.sha ?? null,
          message: data?.message ?? null,
        },
        200,
      );
    } catch (err) {
      const mapped = mapGithubError(err);
      if (!mapped) throw err;
      return c.json({ error: { code: mapped.code, message: mapped.message } }, mapped.status);
    }
  });
}
