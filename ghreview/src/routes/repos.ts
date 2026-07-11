import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { findDocument, listDocuments } from "../db/documents.ts";
import type { AppDeps } from "../deps.ts";
import {
  ErrorSchema,
  PaginationQuerySchema,
  RepoEnvelopeSchema,
  RepoPageSchema,
} from "../schemas.ts";

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
    const page = await listDocuments(deps.db, "repo", { account, limit, cursor });
    return c.json(page, 200);
  });
  app.openapi(getRepo, async (c) => {
    const { owner, repo } = c.req.valid("param");
    const doc = deps.db ? await findDocument(deps.db, "repo", `${owner}/${repo}`) : null;
    if (doc) return c.json(doc, 200);
    return c.json(
      { error: { code: "not_found", message: `Repository ${owner}/${repo} is not synced` } },
      404,
    );
  });
}
