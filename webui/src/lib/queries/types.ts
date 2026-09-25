import type { AccountProvider } from "@bindings/AccountProvider";
import type { OAuthAccount } from "@bindings/OAuthAccount";
import type { SessionAttachment } from "@bindings/SessionAttachment";
import type { UsageWindowView } from "@bindings/UsageWindowView";

export type { AccountModel } from "@bindings/AccountModel";
export type { AccountProvider } from "@bindings/AccountProvider";
export type { AccountUsage } from "@bindings/AccountUsage";
export type { AccountUsageEntry } from "@bindings/AccountUsageEntry";
export type { CreateAccount } from "@bindings/CreateAccount";
export type { CreateProvider } from "@bindings/CreateProvider";
export type { DailyCacheLoss } from "@bindings/DailyCacheLoss";
export type { GrantShare } from "@bindings/GrantShare";
export type { LimitResetResponse } from "@bindings/LimitResetResponse";
export type { LimitResetStatus } from "@bindings/LimitResetStatus";
export type { OAuthAccount } from "@bindings/OAuthAccount";
export type { OAuthFinish } from "@bindings/OAuthFinish";
export type { OAuthStartResponse } from "@bindings/OAuthStartResponse";
export type { RateLimits } from "@bindings/RateLimits";
export type { ResourceShareInfo } from "@bindings/ResourceShareInfo";
export type { SessionAttachment } from "@bindings/SessionAttachment";
export type { SessionBinding } from "@bindings/SessionBinding";
export type { SoftLimitConfig } from "@bindings/SoftLimitConfig";
export type { UpdateAccount } from "@bindings/UpdateAccount";
export type { UpdateProvider } from "@bindings/UpdateProvider";
export type { UsageHistory } from "@bindings/UsageHistory";
export type { UsageHistorySample } from "@bindings/UsageHistorySample";
export type { UsageNotices } from "@bindings/UsageNotices";
export type { UsagePace } from "@bindings/UsagePace";
export type { UsageWindowClose } from "@bindings/UsageWindowClose";
export type { UsageWindowCloses } from "@bindings/UsageWindowCloses";
export type { WastedSummary } from "@bindings/WastedSummary";
export type { AccountShareInfo as ShareInfo } from "@bindings/AccountShareInfo";

/** A usage window as the accounts API renders it, with its server-computed pace. */
export type UsageWindow = UsageWindowView;

/** Single-provider back-compat: the spawn/dispatch pickers derive the
 *  credential from the account's provider-family union; the
 *  remaining first-row reader is DispatchersPanel until it grows a provider
 *  dimension. */
export const primaryProvider = (a: OAuthAccount): AccountProvider | undefined => a.providers[0];

export const ATTACHMENT_CLOCK_SLACK_MS = 60_000;

/** The upload a user message refers to by `name`: the newest one recorded
 *  before the message (plus clock slack), else the earliest of that name. */
export function pickAttachment(
  all: SessionAttachment[],
  name: string,
  messageTs: number,
): SessionAttachment | null {
  const same = all.filter((a) => a.name === name);
  if (same.length === 0) return null;
  const before = same.filter(
    (a) => a.created_at <= messageTs + ATTACHMENT_CLOCK_SLACK_MS,
  );
  if (before.length)
    return before.reduce((best, a) => (a.created_at > best.created_at ? a : best));
  return same.reduce((best, a) => (a.created_at < best.created_at ? a : best));
}

export function attachmentBlobUrl(sessionId: string, hash: string): string {
  return `/api/v1/sessions/${encodeURIComponent(sessionId)}/blobs/${encodeURIComponent(hash)}`;
}
