import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { findDocument, listDocuments } from "../db/documents.ts";
import type { AppDeps } from "../deps.ts";
import {
  ErrorSchema,
  PaginationQuerySchema,
  PullRequestEnvelopeSchema,
  PullRequestPageSchema,
} from "../schemas.ts";

const RepoScope = z.object({
  owner: z.string().openapi({ param: { name: "owner", in: "path" }, example: "DorskFR" }),
  repo: z.string().openapi({ param: { name: "repo", in: "path" }, example: "cctui" }),
});

const PullParams = RepoScope.extend({
  number: z.coerce
    .number()
    .int()
    .positive()
    .openapi({ param: { name: "number", in: "path" }, example: 42 }),
});

const listPulls = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls",
  summary: "List synced pull requests for a repository",
  tags: ["pulls"],
  request: { params: RepoScope, query: PaginationQuerySchema },
  responses: {
    200: {
      description: "A page of pull request envelopes",
      content: { "application/json": { schema: PullRequestPageSchema } },
    },
  },
});

const getPull = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}",
  summary: "Get a synced pull request",
  tags: ["pulls"],
  request: { params: PullParams },
  responses: {
    200: {
      description: "The pull request envelope",
      content: { "application/json": { schema: PullRequestEnvelopeSchema } },
    },
    404: { description: "Not synced", content: { "application/json": { schema: ErrorSchema } } },
  },
});

export function registerPulls(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listPulls, async (c) => {
    const { owner, repo } = c.req.valid("param");
    const { account, limit, cursor } = c.req.valid("query");
    if (!deps.db) return c.json({ items: [], next_cursor: null }, 200);
    const page = await listDocuments(deps.db, "pull_request", {
      account,
      keyPrefix: `${owner}/${repo}#`,
      limit,
      cursor,
    });
    return c.json(page, 200);
  });
  app.openapi(getPull, async (c) => {
    const { owner, repo, number } = c.req.valid("param");
    const doc = deps.db
      ? await findDocument(deps.db, "pull_request", `${owner}/${repo}#${number}`)
      : null;
    if (doc) return c.json(doc, 200);
    return c.json(
      {
        error: {
          code: "not_found",
          message: `Pull request ${owner}/${repo}#${number} is not synced`,
        },
      },
      404,
    );
  });
}
