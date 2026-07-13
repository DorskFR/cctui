import { createRoute, type OpenAPIHono, z } from "@hono/zod-openapi";
import { getUserId } from "../auth/middleware.ts";
import { getDocument, upsertDocument } from "../db/documents.ts";
import { accountOwnedBy } from "../db/notificationState.ts";
import type { AppDeps } from "../deps.ts";
import type { Account } from "../github/account.ts";
import { addPullLabel, type Label, listRepoLabels, removePullLabel } from "../github/labels.ts";
import {
  AccountSchema,
  ErrorSchema,
  LabelMutateSchema,
  PullLabelsSchema,
  RepoLabelListSchema,
} from "../schemas.ts";

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

const PrLabelParams = PrParams.extend({
  name: z.string().openapi({ param: { name: "name", in: "path" }, example: "bug" }),
});

const AccountQuery = z.object({
  account: AccountSchema.openapi({ param: { name: "account", in: "query" } }),
});

const errorResponses = {
  403: {
    description: "Caller does not own the account",
    content: { "application/json": { schema: ErrorSchema } },
  },
  404: {
    description: "Account not managed",
    content: { "application/json": { schema: ErrorSchema } },
  },
};

const listRepoLabelsRoute = createRoute({
  method: "get",
  path: "/v1/repos/{owner}/{repo}/labels",
  summary: "List a repository's labels for the label picker",
  tags: ["labels"],
  request: { params: RepoParams, query: AccountQuery },
  responses: {
    200: {
      description: "The repository's labels",
      content: { "application/json": { schema: RepoLabelListSchema } },
    },
    ...errorResponses,
  },
});

const pullLabelsResponses = {
  200: {
    description: "The pull request's labels",
    content: { "application/json": { schema: PullLabelsSchema } },
  },
  ...errorResponses,
};

const addLabelRoute = createRoute({
  method: "post",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/labels",
  summary: "Add a label to a pull request",
  tags: ["labels"],
  request: {
    params: PrParams,
    body: { content: { "application/json": { schema: LabelMutateSchema } } },
  },
  responses: pullLabelsResponses,
});

const removeLabelRoute = createRoute({
  method: "delete",
  path: "/v1/repos/{owner}/{repo}/pulls/{number}/labels/{name}",
  summary: "Remove a label from a pull request",
  tags: ["labels"],
  request: { params: PrLabelParams, query: AccountQuery },
  responses: pullLabelsResponses,
});

async function patchPullLabels(
  deps: AppDeps,
  account: string,
  owner: string,
  repo: string,
  number: number,
  labels: Label[],
): Promise<void> {
  if (!deps.db) return;
  const key = `${owner}/${repo}#${number}`;
  const doc = await getDocument(deps.db, account, "pull_request", key);
  if (!doc) return;
  const payload = { ...(doc.payload as Record<string, unknown>) };
  payload.labels = labels.map((l) => ({
    name: l.name,
    color: l.color,
    description: l.description,
  }));
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

export function registerLabels(app: OpenAPIHono, deps: AppDeps = {}) {
  app.openapi(listRepoLabelsRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    const items = await listRepoLabels(auth.acct.octokit, p.owner, p.repo);
    return c.json({ items }, 200);
  });

  app.openapi(addLabelRoute, async (c) => {
    const p = c.req.valid("param");
    const { account, name } = c.req.valid("json");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    const labels = await addPullLabel(auth.acct.octokit, p.owner, p.repo, p.number, name);
    await patchPullLabels(deps, account, p.owner, p.repo, p.number, labels);
    return c.json({ labels }, 200);
  });

  app.openapi(removeLabelRoute, async (c) => {
    const p = c.req.valid("param");
    const { account } = c.req.valid("query");
    const auth = await authAccount(deps, getUserId(c), account);
    if (!auth.ok) return c.json({ error: { code: auth.code, message: auth.message } }, auth.status);
    const labels = await removePullLabel(auth.acct.octokit, p.owner, p.repo, p.number, p.name);
    await patchPullLabels(deps, account, p.owner, p.repo, p.number, labels);
    return c.json({ labels }, 200);
  });
}
