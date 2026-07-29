import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { requireOwnedAccount } from "../auth/ownership.ts";
import { findDocument, listDocuments } from "../db/documents.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import {
  AccountSchema,
  ErrorSchema,
  PaginationQuerySchema,
  RepoEnvelopeSchema,
  RepoPageSchema,
} from "../schemas.ts";

const GITHUB_REPOS_PER_PAGE = 100;
const GITHUB_REPOS_MAX_PAGES = 20;

const GithubRepoSchema = z
  .object({
    full_name: z.string().openapi({ example: "DorskFR/cctui" }),
    private: z.boolean().openapi({ example: false }),
    permissions: z
      .object({
        admin: z.boolean().optional(),
        maintain: z.boolean().optional(),
        push: z.boolean().optional(),
        triage: z.boolean().optional(),
        pull: z.boolean().optional(),
      })
      .partial()
      .nullable()
      .openapi({ description: "Caller's permissions on the repo, as returned by GitHub" }),
    pushed_at: z
      .string()
      .nullable()
      .openapi({ example: "2026-07-12T09:00:00Z", description: "Last push time" }),
  })
  .openapi("GithubRepo");

const GithubRepoListSchema = z
  .object({ items: z.array(GithubRepoSchema) })
  .openapi("GithubRepoList");

const GithubReposQuery = z.object({
  account: AccountSchema.openapi({
    param: { name: "account", in: "query" },
    description: "The account/login to list accessible GitHub repos for",
  }),
});

interface GithubRepo {
  full_name: string;
  private: boolean;
  permissions: Record<string, boolean> | null;
  pushed_at: string | null;
}

async function fetchUserRepos(octokit: Account["octokit"]): Promise<GithubRepo[]> {
  const repos: GithubRepo[] = [];
  for (let page = 1; page <= GITHUB_REPOS_MAX_PAGES; page++) {
    const res = await octokit.request("GET /user/repos", {
      affiliation: "owner,collaborator,organization_member",
      per_page: GITHUB_REPOS_PER_PAGE,
      page,
      sort: "pushed",
    });
    const batch = Array.isArray(res.data) ? (res.data as Record<string, unknown>[]) : [];
    for (const repo of batch) {
      repos.push({
        full_name: String(repo.full_name ?? ""),
        private: Boolean(repo.private),
        permissions: (repo.permissions as Record<string, boolean> | undefined) ?? null,
        pushed_at: (repo.pushed_at as string | null | undefined) ?? null,
      });
    }
    if (batch.length < GITHUB_REPOS_PER_PAGE) break;
  }
  return repos;
}

const listGithubRepos = createRoute({
  method: "get",
  path: "/v1/github/repos",
  summary: "List GitHub repos the account can access",
  tags: ["repos"],
  request: { query: GithubReposQuery },
  responses: {
    200: {
      description: "Repos the account's PAT can access, GitHub-shaped for a repo-picker",
      content: { "application/json": { schema: GithubRepoListSchema } },
    },
    403: {
      description: "Caller does not own the account",
      content: { "application/json": { schema: ErrorSchema } },
    },
    404: {
      description: "Account not managed by the sync daemon",
      content: { "application/json": { schema: ErrorSchema } },
    },
  },
});

const RepoParams = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
});

const listRepos = createRoute({
  method: "get",
  path: "/v1/repos",
  summary: "List synced repositories",
  tags: ["repos"],
  request: { query: PaginationQuerySchema },
  responses: {
    200: {
      description: "A page of repository envelopes",
      content: { "application/json": { schema: RepoPageSchema } },
    },
  },
});

const getRepo = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}",
  summary: "Get a synced repository",
  tags: ["repos"],
  request: { params: RepoParams },
  responses: {
    200: {
      description: "The repository envelope",
      content: { "application/json": { schema: RepoEnvelopeSchema } },
    },
    404: { description: "Not synced", content: { "application/json": { schema: ErrorSchema } } },
  },
});

export function registerRepos(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listRepos, async (c) => {
    const { account, limit, cursor } = c.req.valid("query");
    if (!deps.db) return c.json({ items: [], next_cursor: null }, 200);
    const userId = getUserId(c);
    const page = await listDocuments(deps.db, "repo", { account, limit, cursor, userId });
    return c.json(page, 200);
  });
  app.openapi(getRepo, async (c) => {
    const { owner, repo } = c.req.valid("param");
    const userId = getUserId(c);
    const doc = deps.db
      ? await findDocument(deps.db, "repo", `${owner}/${repo}`, { userId })
      : null;
    if (doc) return c.json(doc, 200);
    return c.json(
      { error: { code: "not_found", message: `Repository ${owner}/${repo} is not synced` } },
      404,
    );
  });
  app.openapi(listGithubRepos, async (c) => {
    const { account } = c.req.valid("query");
    const auth = await requireOwnedAccount(deps, c, account);
    if (!auth.ok) return c.json(auth.body, auth.status);
    const items = await fetchUserRepos(auth.acct.octokit);
    return c.json({ items }, 200);
  });
}
