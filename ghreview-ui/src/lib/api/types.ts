import type { components } from "../../generated/api";

export type Schemas = components["schemas"];

export type PullRequestEnvelope = Schemas["PullRequestEnvelope"];
export type RepoEnvelope = Schemas["RepoEnvelope"];
export type NotificationInboxItem = Schemas["NotificationInboxItem"];
export type NotificationInboxPage = Schemas["NotificationInboxPage"];
export type NotificationState = Schemas["NotificationState"];
export type PullRequestPage = Schemas["PullRequestPage"];
export type ViewedStateItem = Schemas["ViewedStateItem"];
export type ViewedStateResult = Schemas["ViewedStateResult"];
export type RepoPage = Schemas["RepoPage"];
export type StatusPayload = Schemas["Status"];
export type SseEvent = Schemas["SseEvent"];

export type ReviewDraft = NonNullable<Schemas["ReviewDraft"]>;
export type ReviewDraftComment = Schemas["ReviewDraftComment"];
export type ReviewSide = ReviewDraftComment["side"];
export type ReviewVerdict = ReviewDraft["verdict"];
export type ReviewDraftResult = Schemas["ReviewDraftResult"];
export type ReviewPublishResult = Schemas["ReviewPublishResult"];
export type ReviewThreadComment = Schemas["ReviewThreadComment"];
export type ReviewThreadList = Schemas["ReviewThreadList"];

export type ReactionSummary = Schemas["ReactionSummary"];
export type ReactionContent = Schemas["ReactionToggle"]["content"];

export type MergeMethod = Schemas["MergePull"]["merge_method"];
export type MergeResult = Schemas["MergeResult"];
export type Reviewer = Schemas["Reviewer"];
export type ReviewerState = Reviewer["state"];
export type ReviewersResult = Schemas["ReviewersResult"];
export type ActivityEvent = Schemas["ActivityEvent"];
export type ActivityList = Schemas["ActivityList"];

export interface ReactionRollup {
  "+1"?: number;
  "-1"?: number;
  laugh?: number;
  hooray?: number;
  confused?: number;
  heart?: number;
  rocket?: number;
  eyes?: number;
  total_count?: number;
}

export type PrState = "open" | "draft" | "merged" | "closed";
export type CiState = "pending" | "success" | "failure" | "none";

export interface GithubUser {
  login: string;
  avatar_url?: string;
}

export interface GithubLabel {
  name: string;
  color?: string;
  description?: string | null;
}

export type Label = Schemas["Label"];

export interface GithubRef {
  ref: string;
  sha: string;
  label?: string;
}

export interface GithubFile {
  filename: string;
  previous_filename?: string;
  status: "added" | "removed" | "modified" | "renamed" | "copied" | "changed" | "unchanged";
  additions: number;
  deletions: number;
  changes: number;
  patch?: string;
  sha?: string;
}

export interface GithubCommit {
  sha?: string;
  commit?: {
    message?: string;
    author?: { name?: string; date?: string } | null;
  };
  author?: GithubUser | null;
}

export interface GithubPull {
  number: number;
  title: string;
  state: "open" | "closed";
  draft?: boolean;
  merged?: boolean;
  merged_at?: string | null;
  mergeable?: boolean | null;
  mergeable_state?: string | null;
  additions?: number;
  deletions?: number;
  changed_files?: number;
  user?: GithubUser | null;
  requested_reviewers?: GithubUser[];
  labels?: GithubLabel[];
  head?: GithubRef;
  base?: GithubRef;
  html_url?: string;
  body?: string | null;
  updated_at?: string;
  files?: GithubFile[];
  commits_list?: GithubCommit[];
  cctui_enriched_head_sha?: string | null;
  ci?: CiState;
  reactions?: ReactionRollup;
}

export interface GithubRepo {
  name: string;
  full_name: string;
  owner?: GithubUser | null;
  description?: string | null;
  private?: boolean;
}

export interface GithubNotificationSubject {
  title: string;
  url: string | null;
  type: string;
}

export interface GithubNotification {
  id: string;
  reason: string;
  unread: boolean;
  updated_at: string;
  subject: GithubNotificationSubject;
  repository?: { full_name: string; name?: string };
}

export function pullOf(env: PullRequestEnvelope): GithubPull {
  return (env.payload ?? {}) as unknown as GithubPull;
}

export function repoOf(env: RepoEnvelope): GithubRepo {
  return (env.payload ?? {}) as unknown as GithubRepo;
}

export function notificationOf(item: NotificationInboxItem): GithubNotification {
  return (item.payload ?? {}) as unknown as GithubNotification;
}

export function prStateOf(pull: GithubPull): PrState {
  if (pull.merged || pull.merged_at) return "merged";
  if (pull.state === "closed") return "closed";
  if (pull.draft) return "draft";
  return "open";
}

export function ciStateOf(pull: GithubPull): CiState {
  return pull.ci ?? "none";
}
