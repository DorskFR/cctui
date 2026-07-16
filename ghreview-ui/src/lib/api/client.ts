import type { components } from "../../generated/api";
import { baseUrl, getToken } from "./config";
import type {
  ActivityList,
  MergeMethod,
  MergeResult,
  NotificationInboxPage,
  NotificationState,
  PullRequestEnvelope,
  PullRequestPage,
  ReactionContent,
  ReactionSummary,
  RepoPage,
  ReviewDraftResult,
  ReviewersResult,
  ReviewPublishResult,
  ReviewSide,
  ReviewThreadList,
  ReviewVerdict,
  StatusPayload,
  ViewedStateResult,
} from "./types";

type Label = components["schemas"]["Label"];

type Schemas = components["schemas"];
export type Subscription = Schemas["Subscription"];
export type SubscriptionKind = Subscription["kind"];
export type GithubRepo = Schemas["GithubRepo"];

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const token = getToken();
  const headers = new Headers(init?.headers);
  headers.set("Accept", "application/json");
  if (init?.body) headers.set("Content-Type", "application/json");
  if (token) headers.set("Authorization", `Bearer ${token}`);

  const res = await fetch(`${baseUrl()}${path}`, { ...init, headers });
  if (!res.ok) {
    let code = "http_error";
    let message = `${res.status} ${res.statusText}`;
    const body = (await res.json().catch(() => null)) as {
      error?: { code?: string; message?: string };
    } | null;
    if (body?.error?.code) code = body.error.code;
    if (body?.error?.message) message = body.error.message;
    throw new ApiError(res.status, code, message);
  }
  if (res.status === 204) return undefined as T;
  return (await res.json()) as T;
}

function qs(params: Record<string, string | number | undefined>): string {
  const usp = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v !== undefined && v !== "") usp.set(k, String(v));
  }
  const s = usp.toString();
  return s ? `?${s}` : "";
}

export interface NotificationFilter {
  account?: string;
  reason?: string;
  repo?: string;
  unread?: "true" | "false";
  undone?: "true" | "false";
  archived?: "true" | "false";
  since?: string;
  limit?: number;
  cursor?: string;
  all?: "true";
}

interface CursorPage<T> {
  items: T[];
  next_cursor: string | null;
}

export async function collectCursorPages<T>(
  fetchPage: (cursor?: string) => Promise<CursorPage<T>>,
): Promise<T[]> {
  const items: T[] = [];
  let cursor: string | undefined;
  do {
    const page = await fetchPage(cursor);
    items.push(...page.items);
    cursor = page.next_cursor ?? undefined;
  } while (cursor);
  return items;
}

export const api = {
  status: () => request<StatusPayload>("/v1/status"),

  repos: (account?: string, limit?: number, cursor?: string) =>
    request<RepoPage>(`/v1/repos${qs({ account, limit, cursor })}`),

  allRepos: (account?: string) =>
    collectCursorPages((cursor) =>
      request<RepoPage>(`/v1/repos${qs({ account, limit: 100, cursor })}`),
    ),

  pulls: (owner: string, repo: string, account?: string, limit?: number, cursor?: string) =>
    request<PullRequestPage>(`/v1/repos/${owner}/${repo}/pulls${qs({ account, limit, cursor })}`),

  allPulls: (owner: string, repo: string, account?: string) =>
    collectCursorPages((cursor) =>
      request<PullRequestPage>(
        `/v1/repos/${owner}/${repo}/pulls${qs({ account, limit: 100, cursor })}`,
      ),
    ),

  pull: (owner: string, repo: string, number: number) =>
    request<PullRequestEnvelope>(`/v1/repos/${owner}/${repo}/pulls/${number}`),

  pullViewed: (owner: string, repo: string, number: number, account: string) =>
    request<ViewedStateResult>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/viewed${qs({ account })}`,
    ),

  setPullViewed: (
    owner: string,
    repo: string,
    number: number,
    account: string,
    paths: string[],
    viewed: boolean,
  ) =>
    request<ViewedStateResult>(`/v1/repos/${owner}/${repo}/pulls/${number}/viewed`, {
      method: "PUT",
      body: JSON.stringify({ account, paths, viewed }),
    }),

  notifications: (filter: NotificationFilter = {}) =>
    request<NotificationInboxPage>(`/v1/notifications${qs({ ...filter })}`),

  setNotificationState: (
    account: string,
    threadIds: string[],
    patch: { read?: boolean; done?: boolean; archived?: boolean },
  ) =>
    request<{ items: { thread_id: string; state: NotificationState }[] }>(
      "/v1/notifications/state",
      { method: "POST", body: JSON.stringify({ account, thread_ids: threadIds, ...patch }) },
    ),

  listSubscriptions: (account?: string) =>
    request<{ items: Subscription[] }>(`/v1/subscriptions${qs({ account })}`),

  subscribe: (target: string, kind: SubscriptionKind = "pull_request", account?: string) =>
    request<Subscription>("/v1/subscriptions", {
      method: "POST",
      body: JSON.stringify({ kind, target, account }),
    }),

  unsubscribe: (id: string) =>
    request<void>(`/v1/subscriptions/${encodeURIComponent(id)}`, { method: "DELETE" }),

  githubRepos: (account: string) =>
    request<{ items: GithubRepo[] }>(`/v1/github/repos${qs({ account })}`),

  forceSync: (account?: string) =>
    request<{ account: string; status: "ok" }>("/v1/sync", {
      method: "POST",
      body: JSON.stringify({ account }),
    }),

  reviewDraft: (owner: string, repo: string, number: number, account: string) =>
    request<ReviewDraftResult>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/review-draft${qs({ account })}`,
    ),

  addReviewComment: (
    owner: string,
    repo: string,
    number: number,
    input: {
      account: string;
      path: string;
      side: ReviewSide;
      line: number;
      start_line?: number | null;
      start_side?: ReviewSide | null;
      body: string;
      head_sha?: string;
    },
  ) =>
    request<ReviewDraftResult>(`/v1/repos/${owner}/${repo}/pulls/${number}/review-draft/comments`, {
      method: "POST",
      body: JSON.stringify(input),
    }),

  editReviewComment: (
    owner: string,
    repo: string,
    number: number,
    commentId: string,
    input: { account: string; body?: string; line?: number; side?: ReviewSide },
  ) =>
    request<ReviewDraftResult>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/review-draft/comments/${encodeURIComponent(commentId)}`,
      { method: "PATCH", body: JSON.stringify(input) },
    ),

  deleteReviewComment: (
    owner: string,
    repo: string,
    number: number,
    commentId: string,
    account: string,
  ) =>
    request<ReviewDraftResult>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/review-draft/comments/${encodeURIComponent(commentId)}${qs({ account })}`,
      { method: "DELETE" },
    ),

  publishReview: (
    owner: string,
    repo: string,
    number: number,
    input: { account: string; verdict: ReviewVerdict; body: string },
  ) =>
    request<ReviewPublishResult>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/review-draft/publish`,
      { method: "POST", body: JSON.stringify(input) },
    ),

  reviewThreads: (owner: string, repo: string, number: number, account: string) =>
    request<ReviewThreadList>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/comments${qs({ account })}`,
    ),

  deletePublishedReviewComment: (owner: string, repo: string, commentId: number, account: string) =>
    request<{ deleted: boolean }>(
      `/v1/repos/${owner}/${repo}/pulls/comments/${commentId}${qs({ account })}`,
      { method: "DELETE" },
    ),

  deleteIssueComment: (owner: string, repo: string, commentId: number, account: string) =>
    request<{ deleted: boolean }>(
      `/v1/repos/${owner}/${repo}/issues/comments/${commentId}${qs({ account })}`,
      { method: "DELETE" },
    ),

  togglePullReaction: (
    owner: string,
    repo: string,
    number: number,
    account: string,
    content: ReactionContent,
  ) =>
    request<ReactionSummary>(`/v1/repos/${owner}/${repo}/pulls/${number}/reactions`, {
      method: "POST",
      body: JSON.stringify({ account, content }),
    }),

  toggleIssueCommentReaction: (
    owner: string,
    repo: string,
    commentId: number,
    account: string,
    content: ReactionContent,
  ) =>
    request<ReactionSummary>(`/v1/repos/${owner}/${repo}/issues/comments/${commentId}/reactions`, {
      method: "POST",
      body: JSON.stringify({ account, content }),
    }),

  toggleReviewCommentReaction: (
    owner: string,
    repo: string,
    commentId: number,
    account: string,
    content: ReactionContent,
  ) =>
    request<ReactionSummary>(`/v1/repos/${owner}/${repo}/pulls/comments/${commentId}/reactions`, {
      method: "POST",
      body: JSON.stringify({ account, content }),
    }),

  mergePull: (
    owner: string,
    repo: string,
    number: number,
    input: { account: string; merge_method: MergeMethod; expected_head_sha?: string },
  ) =>
    request<MergeResult>(`/v1/repos/${owner}/${repo}/pulls/${number}/merge`, {
      method: "POST",
      body: JSON.stringify(input),
    }),

  reviewers: (owner: string, repo: string, number: number, account: string) =>
    request<ReviewersResult>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/reviewers${qs({ account })}`,
    ),

  reRequestReviewers: (
    owner: string,
    repo: string,
    number: number,
    account: string,
    reviewers: string[],
  ) =>
    request<ReviewersResult>(`/v1/repos/${owner}/${repo}/pulls/${number}/reviewers/re-request`, {
      method: "POST",
      body: JSON.stringify({ account, reviewers }),
    }),

  requestReviewers: (
    owner: string,
    repo: string,
    number: number,
    account: string,
    reviewers: string[],
    teamReviewers: string[] = [],
  ) =>
    request<ReviewersResult>(`/v1/repos/${owner}/${repo}/pulls/${number}/reviewers/request`, {
      method: "POST",
      body: JSON.stringify({ account, reviewers, team_reviewers: teamReviewers }),
    }),

  activity: (owner: string, repo: string, number: number, account: string) =>
    request<ActivityList>(`/v1/repos/${owner}/${repo}/pulls/${number}/activity${qs({ account })}`),

  repoLabels: (owner: string, repo: string, account: string) =>
    request<{ items: Label[] }>(`/v1/repos/${owner}/${repo}/labels${qs({ account })}`),

  addPullLabel: (owner: string, repo: string, number: number, account: string, name: string) =>
    request<{ labels: Label[] }>(`/v1/repos/${owner}/${repo}/pulls/${number}/labels`, {
      method: "POST",
      body: JSON.stringify({ account, name }),
    }),

  removePullLabel: (owner: string, repo: string, number: number, account: string, name: string) =>
    request<{ labels: Label[] }>(
      `/v1/repos/${owner}/${repo}/pulls/${number}/labels/${encodeURIComponent(name)}${qs({ account })}`,
      { method: "DELETE" },
    ),
};
