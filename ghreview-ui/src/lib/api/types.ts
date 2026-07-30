import type { components } from "../../generated/api";

export type Schemas = components["schemas"];

export type PullRequestEnvelope = Schemas["PullRequestEnvelope"];
export type RepoEnvelope = Schemas["RepoEnvelope"];
export type NotificationInboxItem = Schemas["NotificationInboxItem"];
export type NotificationInboxPage = Schemas["NotificationInboxPage"];
export type NotificationState = Schemas["NotificationState"];
export type PullRequestPage = Schemas["PullRequestPage"];
export type SnoozedPull = Schemas["SnoozedPull"];
export type SnoozedPullList = Schemas["SnoozedPullList"];
export type SnoozeResult = Schemas["SnoozeResult"];
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

function asRecord(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

export function isGithubPull(value: unknown): value is GithubPull {
  const v = asRecord(value);
  if (!v) return false;
  return (
    typeof v.number === "number" &&
    Number.isFinite(v.number) &&
    typeof v.title === "string" &&
    (v.state === "open" || v.state === "closed")
  );
}

export function isGithubRepo(value: unknown): value is GithubRepo {
  const v = asRecord(value);
  return v !== null && typeof v.full_name === "string" && typeof v.name === "string";
}

export function isGithubNotification(value: unknown): value is GithubNotification {
  const v = asRecord(value);
  if (!v) return false;
  const subject = asRecord(v.subject);
  return (
    typeof v.id === "string" &&
    typeof v.reason === "string" &&
    subject !== null &&
    typeof subject.title === "string" &&
    typeof subject.type === "string" &&
    (typeof subject.url === "string" || subject.url === null)
  );
}

const UNKNOWN_PULL: GithubPull = { number: 0, title: "", state: "open" };
const UNKNOWN_REPO: GithubRepo = { name: "", full_name: "" };
const UNKNOWN_NOTIFICATION: GithubNotification = {
  id: "",
  reason: "",
  unread: false,
  updated_at: "",
  subject: { title: "", url: null, type: "" },
};

export function asGithubPull(payload: unknown): GithubPull {
  return isGithubPull(payload) ? payload : UNKNOWN_PULL;
}

export function pullOf(env: PullRequestEnvelope): GithubPull {
  return asGithubPull(env.payload);
}

export function repoOf(env: RepoEnvelope): GithubRepo {
  return isGithubRepo(env.payload) ? env.payload : UNKNOWN_REPO;
}

export function notificationOf(item: NotificationInboxItem): GithubNotification {
  return isGithubNotification(item.payload) ? item.payload : UNKNOWN_NOTIFICATION;
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
