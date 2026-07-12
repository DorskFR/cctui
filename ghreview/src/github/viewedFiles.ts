import type { GraphqlClient } from "../graphql/client.ts";

export interface ViewedPushResult {
  ok: boolean;
  error?: string;
}

interface GraphqlLikeError {
  status?: number;
  errors?: { type?: string; message?: string }[];
  message?: string;
}

// A 404-shaped GraphQL error (PR/file gone) is terminal, not retryable, so it is
// reported ok — mirroring markThreadRead's treatment of a vanished thread.
function isGone(err: GraphqlLikeError): boolean {
  if (err.status === 404) return true;
  return (err.errors ?? []).some((e) => e.type === "NOT_FOUND");
}

export async function pushFileViewed(
  client: GraphqlClient,
  pullRequestId: string,
  path: string,
  viewed: boolean,
): Promise<ViewedPushResult> {
  try {
    if (viewed) await client.markFileViewed({ pullRequestId, path });
    else await client.unmarkFileViewed({ pullRequestId, path });
    return { ok: true };
  } catch (err) {
    const e = err as GraphqlLikeError;
    if (isGone(e)) return { ok: true };
    const detail = e.message ?? e.errors?.[0]?.message ?? "unknown error";
    return { ok: false, error: `viewed push failed: ${detail}` };
  }
}

export interface GithubViewedState {
  pullRequestId: string | null;
  viewedByPath: Map<string, boolean>;
}

export async function fetchGithubViewedState(
  client: GraphqlClient,
  owner: string,
  repo: string,
  number: number,
): Promise<GithubViewedState> {
  const viewedByPath = new Map<string, boolean>();
  let pullRequestId: string | null = null;
  let cursor: string | null = null;
  for (let page = 0; page < 50; page++) {
    const res = await client.pullViewedFiles({ owner, repo, number, cursor });
    const pr = res.repository?.pullRequest;
    if (!pr) break;
    pullRequestId = pr.id;
    for (const node of pr.files?.nodes ?? []) {
      if (node) viewedByPath.set(node.path, node.viewerViewedState === "VIEWED");
    }
    if (!pr.files?.pageInfo.hasNextPage) break;
    cursor = pr.files.pageInfo.endCursor;
  }
  return { pullRequestId, viewedByPath };
}
