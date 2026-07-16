import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { accountOwnedBy } from "../db/notificationState.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
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

export type ReviewState = "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" | "PENDING";

interface RawReview {
  user: string | null;
  avatar_url: string | null;
  state: string;
}

const VERDICTS = new Set(["APPROVED", "CHANGES_REQUESTED", "DISMISSED"]);

export function reduceReviewStates(
  reviews: RawReview[],
): Map<string, { avatar_url: string | null; state: ReviewState }> {
  const out = new Map<string, { avatar_url: string | null; state: ReviewState }>();
  for (const r of reviews) {
    if (!r.user) continue;
    const state = r.state.toUpperCase();
    const prev = out.get(r.user);
    const avatar_url = r.avatar_url ?? prev?.avatar_url ?? null;
    if (VERDICTS.has(state)) {
      out.set(r.user, { avatar_url, state: state as ReviewState });
    } else if (state === "COMMENTED") {
      if (!prev || prev.state === "COMMENTED") {
        out.set(r.user, { avatar_url, state: "COMMENTED" });
      } else {
        out.set(r.user, { avatar_url, state: prev.state });
      }
    }
  }
  return out;
}

async function fetchReviews(
  octokit: Account["octokit"],
  p: {
    owner: string;
    repo: string;
    number: number;
  },
): Promise<RawReview[]> {
  const reviews: RawReview[] = [];
  for (let page = 1; page <= 20; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/reviews", {
      owner: p.owner,
      repo: p.repo,
      pull_number: p.number,
      per_page: 100,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as Record<string, unknown>[]) : [];
    for (const rv of batch) {
      const user = (rv.user as { login?: string; avatar_url?: string } | undefined) ?? undefined;
      reviews.push({
        user: user?.login ?? null,
        avatar_url: user?.avatar_url ?? null,
        state: String(rv.state ?? ""),
      });
    }
    if (batch.length < 100) break;
  }
  return reviews;
}

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
    fetchReviews(octokit, p),
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

async function authAccount(
  deps: AppDeps,
  uid: string | undefined,
  account: string,
): Promise<
  | { ok: true; acct: Account }
  | { ok: false; code: "forbidden" | "not_found"; message: string; status: 403 | 404 }
> {
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

export function registerReviewers(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(getReviewersRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    return c.json(await buildResult(auth.acct.octokit, p), 200);
  });

  app.openapi(reRequestRoute, async (c) => {
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const auth = await authAccount(deps, getUserId(c), body.account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    await auth.acct.octokit.request(
      "POST /repos/{owner}/{repo}/pulls/{pull_number}/requested_reviewers",
      { owner: p.owner, repo: p.repo, pull_number: p.number, reviewers: body.reviewers },
    );
    return c.json(await buildResult(auth.acct.octokit, p), 200);
  });

  app.openapi(requestRoute, async (c) => {
    const p = c.req.valid("param");
    const body = c.req.valid("json");
    const auth = await authAccount(deps, getUserId(c), body.account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
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
