import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { requireOwnedAccount } from "../auth/ownership.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import { fetchPullReviews, type ReviewState, reduceReviewStates } from "../github/reviews.ts";
import {
  ErrorSchema,
  RequestReviewersSchema,
  ReRequestReviewersSchema,
  ReviewersResultSchema,
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

const AccountQuery = z.object({
  account: z
    .string()
    .min(1)
    .openapi({ param: { name: "account", in: "query" }, example: "DorskFR" }),
});

interface RequestedInfo {
  reviewers: { login: string; avatar_url: string | null }[];
  teams: { name: string; slug: string }[];
}

async function fetchRequested(
  octokit: Account["octokit"],
  p: {
    owner: string;
    repo: string;
    number: number;
  },
): Promise<RequestedInfo> {
  const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}", {
    owner: p.owner,
    repo: p.repo,
    pull_number: p.number,
  });
  const data = res.data as {
    requested_reviewers?: { login?: string; avatar_url?: string }[];
    requested_teams?: { name?: string; slug?: string }[];
  };
  return {
    reviewers: (data.requested_reviewers ?? [])
      .filter((r) => r.login)
      .map((r) => ({ login: r.login as string, avatar_url: r.avatar_url ?? null })),
    teams: (data.requested_teams ?? [])
      .filter((t) => t.slug)
      .map((t) => ({ name: t.name ?? (t.slug as string), slug: t.slug as string })),
  };
}

const getReviewersRoute = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/reviewers",
  summary: "List requested reviewers, teams, and each reviewer's latest review state",
  tags: ["pulls"],
  request: { params: PullParams, query: AccountQuery },
  responses: {
    200: {
      description: "Reviewer states",
      content: { "application/json": { schema: ReviewersResultSchema } },
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

const reRequestRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/reviewers/re-request",
  summary: "Re-request a review from one or more reviewers",
  tags: ["pulls"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: ReRequestReviewersSchema } } },
  },
  responses: {
    200: {
      description: "Updated reviewer states",
      content: { "application/json": { schema: ReviewersResultSchema } },
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

const requestRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/reviewers/request",
  summary: "Request a review from new reviewers and/or teams",
  tags: ["pulls"],
  request: {
    params: PullParams,
    body: { content: { "application/json": { schema: RequestReviewersSchema } } },
  },
  responses: {
    200: {
      description: "Updated reviewer states",
      content: { "application/json": { schema: ReviewersResultSchema } },
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

async function buildResult(
  octokit: Account["octokit"],
  p: { owner: string; repo: string; number: number },
): Promise<z.infer<typeof ReviewersResultSchema>> {
  const [requested, reviews] = await Promise.all([
    fetchRequested(octokit, p),
    fetchPullReviews(octokit, p),
  ]);
  const states = reduceReviewStates(reviews);
  const requestedLogins = new Set(requested.reviewers.map((r) => r.login));

  const logins = new Set<string>([...states.keys(), ...requestedLogins]);
  const reviewers = [...logins].map((login) => {
    const reviewed = states.get(login);
    const req = requested.reviewers.find((r) => r.login === login);
    return {
      login,
      avatar_url: reviewed?.avatar_url ?? req?.avatar_url ?? null,
      state: reviewed?.state ?? ("PENDING" as ReviewState),
      requested: requestedLogins.has(login),
    };
  });
  reviewers.sort((a, b) => a.login.localeCompare(b.login));
  return { reviewers, requested_teams: requested.teams };
}

export function registerReviewers(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(getReviewersRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const auth = await requireOwnedAccount(deps, c, account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    return c.json(await buildResult(auth.acct.octokit, p), 200);
  });

  app.openapi(reRequestRoute, async (c) => {
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const auth = await requireOwnedAccount(deps, c, body.account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    await auth.acct.octokit.request(
      "POST /repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers",
      { owner: p.owner, repo: p.repo, pull_number: p.number, reviewers: body.reviewers },
    );
    return c.json(await buildResult(auth.acct.octokit, p), 200);
  });

  app.openapi(requestRoute, async (c) => {
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const auth = await requireOwnedAccount(deps, c, body.account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    await auth.acct.octokit.request(
      "POST /repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers",
      {
        owner: p.owner,
        repo: p.repo,
        pull_number: p.number,
        reviewers: body.reviewers,
        team_reviewers: body.team_reviewers,
      },
    );
    return c.json(await buildResult(auth.acct.octokit, p), 200);
  });
}
