import type { Account } from "../github/account.ts";
import { fetchPullReviews, reduceReviewStates } from "../github/reviews.ts";

const PULL_FILES_PER_PAGE = 100;
const PULL_FILES_MAX_PAGES = 30;

const PULL_COMMITS_PER_PAGE = 100;
const PULL_COMMITS_MAX_PAGES = 30;

interface PullFileStat {
  additions?: unknown;
  deletions?: unknown;
}

export interface PullStats {
  additions: number;
  deletions: number;
  changed_files: number;
}

export type ReviewDecision = "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED" | null;

export function pullStatsFromFiles(files: unknown[]): PullStats {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    const stat = f as PullFileStat;
    if (typeof stat.additions === "number") additions += stat.additions;
    if (typeof stat.deletions === "number") deletions += stat.deletions;
  }
  return { additions, deletions, changed_files: files.length };
}

export function enrichPullStats(
  payload: Record<string, unknown>,
  files: unknown[],
): Record<string, unknown> {
  const stats = pullStatsFromFiles(files);
  return {
    ...payload,
    additions: typeof payload.additions === "number" ? payload.additions : stats.additions,
    deletions: typeof payload.deletions === "number" ? payload.deletions : stats.deletions,
    changed_files:
      typeof payload.changed_files === "number" ? payload.changed_files : stats.changed_files,
  };
}

export async function fetchPullFiles(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
): Promise<unknown[]> {
  const files: unknown[] = [];
  for (let page = 1; page <= PULL_FILES_MAX_PAGES; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/files", {
      owner,
      repo,
      pull_number: number,
      per_page: PULL_FILES_PER_PAGE,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as unknown[]) : [];
    files.push(...batch);
    if (batch.length < PULL_FILES_PER_PAGE) break;
  }
  return files;
}

export async function fetchPullCommits(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
): Promise<unknown[]> {
  const commits: unknown[] = [];
  for (let page = 1; page <= PULL_COMMITS_MAX_PAGES; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/commits", {
      owner,
      repo,
      pull_number: number,
      per_page: PULL_COMMITS_PER_PAGE,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as unknown[]) : [];
    commits.push(...batch);
    if (batch.length < PULL_COMMITS_PER_PAGE) break;
  }
  return commits;
}

function countRequested(payload: Record<string, unknown>): number {
  const reviewers = Array.isArray(payload.requested_reviewers) ? payload.requested_reviewers : [];
  const teams = Array.isArray(payload.requested_teams) ? payload.requested_teams : [];
  return reviewers.length + teams.length;
}

export function deriveReviewDecision(
  reviewStates: Iterable<string>,
  requestedCount: number,
): ReviewDecision {
  let hasChanges = false;
  let hasApproved = false;
  for (const state of reviewStates) {
    if (state === "CHANGES_REQUESTED") hasChanges = true;
    else if (state === "APPROVED") hasApproved = true;
  }
  if (hasChanges) return "CHANGES_REQUESTED";
  if (requestedCount > 0) return "REVIEW_REQUIRED";
  if (hasApproved) return "APPROVED";
  return null;
}

async function computeReviewDecision(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
  payload: Record<string, unknown>,
): Promise<ReviewDecision> {
  const reviews = await fetchPullReviews(octokit, { owner, repo, number });
  const states = reduceReviewStates(reviews);
  const stateList = [...states.values()].map((v) => v.state);
  return deriveReviewDecision(stateList, countRequested(payload));
}

export function pullHeadSha(payload: Record<string, unknown>): string | null {
  const head = payload.head as { sha?: unknown } | undefined;
  return typeof head?.sha === "string" ? head.sha : null;
}

export function needsPullEnrichment(payload: Record<string, unknown>): boolean {
  const headSha = pullHeadSha(payload);
  return (
    !Array.isArray(payload.files) ||
    !Array.isArray(payload.commits_list) ||
    (headSha !== null && payload.cctui_enriched_head_sha !== headSha)
  );
}

export async function enrichPullPayload(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
  payload: Record<string, unknown>,
): Promise<Record<string, unknown>> {
  let enriched = payload;
  if (needsPullEnrichment(payload)) {
    const [files, commits] = await Promise.all([
      fetchPullFiles(octokit, owner, repo, number),
      fetchPullCommits(octokit, owner, repo, number),
    ]);
    enriched = enrichPullStats(payload, files);
    enriched.files = files;
    enriched.commits_list = commits;
    enriched.cctui_enriched_head_sha = pullHeadSha(payload);
  }
  enriched.review_decision = await computeReviewDecision(octokit, owner, repo, number, enriched);
  return enriched;
}
