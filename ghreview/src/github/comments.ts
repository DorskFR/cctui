import type { Account } from "./account.ts";

function parseTrailingNumber(url: string | null | undefined): number | null {
  if (typeof url !== "string") return null;
  const match = /\/(\d+)(?:$|[?#])/.exec(url);
  return match ? Number(match[1]) : null;
}

async function parentNumber(
  octokit: Account["octokit"],
  route: string,
  field: string,
): Promise<number | null> {
  try {
    const res = await octokit.request(route);
    const data = res.data as Record<string, unknown> | null;
    return parseTrailingNumber(data?.[field] as string | undefined);
  } catch {
    return null;
  }
}

export async function deletePullReviewComment(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  commentId: number,
): Promise<number | null> {
  const number = await parentNumber(
    octokit,
    `GET /repos/${owner}/${repo}/pulls/comments/${commentId}`,
    "pull_request_url",
  );
  await octokit.request(`DELETE /repos/${owner}/${repo}/pulls/comments/${commentId}`);
  return number;
}

export async function deleteIssueComment(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  commentId: number,
): Promise<number | null> {
  const number = await parentNumber(
    octokit,
    `GET /repos/${owner}/${repo}/issues/comments/${commentId}`,
    "issue_url",
  );
  await octokit.request(`DELETE /repos/${owner}/${repo}/issues/comments/${commentId}`);
  return number;
}
