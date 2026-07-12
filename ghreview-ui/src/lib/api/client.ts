import { baseUrl, getToken } from "./config";
import type {
  NotificationInboxPage,
  NotificationState,
  PullRequestEnvelope,
  PullRequestPage,
  RepoPage,
  StatusPayload,
  ViewedStateResult,
} from "./types";

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
}

export const api = {
  status: () => request<StatusPayload>("/v1/status"),

  repos: (account?: string, limit?: number, cursor?: string) =>
    request<RepoPage>(`/v1/repos${qs({ account, limit, cursor })}`),

  pulls: (owner: string, repo: string, account?: string, limit?: number, cursor?: string) =>
    request<PullRequestPage>(`/v1/repos/${owner}/${repo}/pulls${qs({ account, limit, cursor })}`),

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
};
