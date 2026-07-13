import { QueryClient } from "@tanstack/svelte-query";
import type { NotificationFilter } from "./client";
import { api } from "./client";

export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 30_000,
      gcTime: 5 * 60_000,
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

export const keys = {
  status: () => ["status"] as const,
  repos: (account?: string) => ["repos", account ?? "*"] as const,
  pulls: (owner: string, repo: string, account?: string) =>
    ["pulls", owner, repo, account ?? "*"] as const,
  pull: (owner: string, repo: string, number: number) => ["pull", owner, repo, number] as const,
  pullViewed: (owner: string, repo: string, number: number) =>
    ["pull-viewed", owner, repo, number] as const,
  reviewDraft: (owner: string, repo: string, number: number) =>
    ["review-draft", owner, repo, number] as const,
  reviewThreads: (owner: string, repo: string, number: number) =>
    ["review-threads", owner, repo, number] as const,
  reviewers: (owner: string, repo: string, number: number) =>
    ["reviewers", owner, repo, number] as const,
  activity: (owner: string, repo: string, number: number) =>
    ["activity", owner, repo, number] as const,
  notifications: (filter: NotificationFilter) => ["notifications", JSON.stringify(filter)] as const,
};

export const queries = {
  status: () => ({ queryKey: keys.status(), queryFn: () => api.status() }),
  repos: (account?: string) => ({
    queryKey: keys.repos(account),
    queryFn: () => api.repos(account),
  }),
  pulls: (owner: string, repo: string, account?: string) => ({
    queryKey: keys.pulls(owner, repo, account),
    queryFn: () => api.pulls(owner, repo, account),
  }),
  pull: (owner: string, repo: string, number: number) => ({
    queryKey: keys.pull(owner, repo, number),
    queryFn: () => api.pull(owner, repo, number),
  }),
  notifications: (filter: NotificationFilter) => ({
    queryKey: keys.notifications(filter),
    queryFn: () => api.notifications(filter),
  }),
};
