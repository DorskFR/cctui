import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { accountOwnedBy } from "../db/notificationState.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import { ActivityListSchema, ErrorSchema } from "../schemas.ts";

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

export type ActivityEvent = z.infer<typeof ActivityListSchema>["items"][number];

const RENDERABLE = new Set([
  "committed",
  "reviewed",
  "commented",
  "labeled",
  "unlabeled",
  "review_requested",
  "review_request_removed",
  "assigned",
  "unassigned",
  "head_ref_force_pushed",
  "merged",
  "closed",
  "reopened",
  "renamed",
]);

const EXCERPT_LEN = 280;

function excerpt(body: unknown): string | undefined {
  if (typeof body !== "string") return undefined;
  const trimmed = body.trim();
  if (!trimmed) return undefined;
  return trimmed.length > EXCERPT_LEN ? `${trimmed.slice(0, EXCERPT_LEN)}…` : trimmed;
}

function actorOf(raw: Record<string, unknown>): ActivityEvent["actor"] {
  const src = (raw.actor ?? raw.user) as { login?: string; avatar_url?: string } | undefined;
  if (!src?.login) return null;
  return { login: src.login, avatar_url: src.avatar_url ?? null };
}

function shortSha(sha: unknown): string | undefined {
  return typeof sha === "string" && sha.length > 0 ? sha.slice(0, 7) : undefined;
}

export function normalizeTimelineEvent(raw: Record<string, unknown>): ActivityEvent | null {
  const event = typeof raw.event === "string" ? raw.event : "";
  if (!RENDERABLE.has(event)) return null;

  const created_at =
    (raw.created_at as string | undefined) ??
    (raw.submitted_at as string | undefined) ??
    (raw.author as { date?: string } | undefined)?.date ??
    null;

  const base: ActivityEvent = { event, actor: actorOf(raw), created_at: created_at ?? null };
  const detail: NonNullable<ActivityEvent["detail"]> = {};

  switch (event) {
    case "committed": {
      const sha = shortSha(raw.sha);
      if (sha) detail.sha = sha;
      const message = excerpt((raw.message as string | undefined)?.split("\n")[0]);
      if (message) detail.message = message;
      const author = raw.author as { name?: string } | undefined;
      if (author?.name) detail.author_name = author.name;
      break;
    }
    case "reviewed": {
      if (typeof raw.state === "string") detail.state = raw.state.toUpperCase();
      const body = excerpt(raw.body);
      if (body) detail.body = body;
      break;
    }
    case "commented": {
      const body = excerpt(raw.body);
      if (body) detail.body = body;
      break;
    }
    case "labeled":
    case "unlabeled": {
      const label = raw.label as { name?: string; color?: string } | undefined;
      if (label?.name) detail.label = { name: label.name, color: label.color ?? null };
      break;
    }
    case "review_requested":
    case "review_request_removed": {
      const reviewer = raw.requested_reviewer as
        | { login?: string; avatar_url?: string }
        | undefined;
      if (reviewer?.login) {
        detail.reviewer = { login: reviewer.login, avatar_url: reviewer.avatar_url ?? null };
      }
      const team = raw.requested_team as { name?: string; slug?: string } | undefined;
      if (team?.slug) detail.team = team.name ?? team.slug;
      break;
    }
    case "assigned":
    case "unassigned": {
      const assignee = raw.assignee as { login?: string; avatar_url?: string } | undefined;
      if (assignee?.login) {
        detail.assignee = { login: assignee.login, avatar_url: assignee.avatar_url ?? null };
      }
      break;
    }
    case "merged":
    case "closed": {
      const sha = shortSha(raw.commit_id);
      if (sha) detail.sha = sha;
      break;
    }
    case "renamed": {
      const rename = raw.rename as { from?: string; to?: string } | undefined;
      if (rename?.from) detail.from = rename.from;
      if (rename?.to) detail.to = rename.to;
      break;
    }
    default:
      break;
  }

  if (Object.keys(detail).length > 0) base.detail = detail;
  return base;
}

const NEXT_RE = /(?:^|,)\s*<[^>]*>;\s*rel="next"/;

async function fetchTimeline(
  octokit: Account["octokit"],
  p: { owner: string; repo: string; number: number },
): Promise<ActivityEvent[]> {
  const events: ActivityEvent[] = [];
  for (let page = 1; page <= 20; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/issues/{issue_number}/timeline", {
      owner: p.owner,
      repo: p.repo,
      issue_number: p.number,
      per_page: 100,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as Record<string, unknown>[]) : [];
    for (const raw of batch) {
      const normalized = normalizeTimelineEvent(raw);
      if (normalized) events.push(normalized);
    }
    if (!NEXT_RE.test(res.headers.link ?? "")) break;
  }
  return events;
}

const getActivityRoute = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/activity",
  summary: "Chronological activity timeline for a pull request",
  tags: ["pulls"],
  request: { params: PullParams, query: AccountQuery },
  responses: {
    200: {
      description: "Normalized activity timeline",
      content: { "application/json": { schema: ActivityListSchema } },
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

export function registerActivity(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(getActivityRoute, async (c) => {
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
    if (!acct) {
      return c.json(
        { error: { code: "not_found", message: `Account ${account} is not managed` } },
        404,
      );
    }
    const items = await fetchTimeline(acct.octokit, p);
    return c.json({ items }, 200);
  });
}
