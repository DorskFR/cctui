import { describe, expect, it } from "vitest";
import {
  asGithubPull,
  isGithubNotification,
  isGithubPull,
  isGithubRepo,
  notificationOf,
  pullOf,
  repoOf,
} from "./types";
import type { NotificationInboxItem, PullRequestEnvelope, RepoEnvelope } from "./types";

const pullPayload = { number: 7, title: "Add tests", state: "open", draft: false };
const repoPayload = { name: "cctui", full_name: "DorskFR/cctui" };
const notificationPayload = {
  id: "42",
  reason: "review_requested",
  unread: true,
  updated_at: "2026-07-30T10:00:00Z",
  subject: { title: "Add tests", url: "https://api.github.com/repos/o/r/pulls/7", type: "PullRequest" },
};

function envelope<T>(payload: unknown): T {
  return { account: "DorskFR", payload } as T;
}

describe("payload guards", () => {
  it("accepts well-formed GitHub payloads", () => {
    expect(isGithubPull(pullPayload)).toBe(true);
    expect(isGithubRepo(repoPayload)).toBe(true);
    expect(isGithubNotification(notificationPayload)).toBe(true);
    expect(isGithubNotification({ ...notificationPayload, subject: { title: "t", url: null, type: "Issue" } })).toBe(
      true,
    );
  });

  it("rejects malformed pull payloads", () => {
    expect(isGithubPull(null)).toBe(false);
    expect(isGithubPull("nope")).toBe(false);
    expect(isGithubPull([pullPayload])).toBe(false);
    expect(isGithubPull({ ...pullPayload, number: "7" })).toBe(false);
    expect(isGithubPull({ ...pullPayload, number: Number.NaN })).toBe(false);
    expect(isGithubPull({ ...pullPayload, title: undefined })).toBe(false);
    expect(isGithubPull({ ...pullPayload, state: "merged" })).toBe(false);
  });

  it("rejects malformed repo payloads", () => {
    expect(isGithubRepo(undefined)).toBe(false);
    expect(isGithubRepo({ full_name: "DorskFR/cctui" })).toBe(false);
    expect(isGithubRepo({ name: "cctui", full_name: 7 })).toBe(false);
  });

  it("rejects malformed notification payloads", () => {
    expect(isGithubNotification({})).toBe(false);
    expect(isGithubNotification({ ...notificationPayload, id: 42 })).toBe(false);
    expect(isGithubNotification({ ...notificationPayload, subject: null })).toBe(false);
    expect(isGithubNotification({ ...notificationPayload, subject: { title: "t", type: "Issue" } })).toBe(false);
    expect(isGithubNotification({ ...notificationPayload, subject: { title: "t", url: 3, type: "Issue" } })).toBe(
      false,
    );
  });
});

describe("payload narrowing at the envelope boundary", () => {
  it("passes validated payloads through untouched", () => {
    expect(pullOf(envelope<PullRequestEnvelope>(pullPayload)).number).toBe(7);
    expect(repoOf(envelope<RepoEnvelope>(repoPayload)).full_name).toBe("DorskFR/cctui");
    expect(notificationOf(envelope<NotificationInboxItem>(notificationPayload)).id).toBe("42");
    expect(asGithubPull(pullPayload).title).toBe("Add tests");
  });

  it("substitutes an inert value for payloads that fail validation", () => {
    expect(pullOf(envelope<PullRequestEnvelope>(null))).toEqual({
      number: 0,
      title: "",
      state: "open",
    });
    expect(pullOf(envelope<PullRequestEnvelope>({ number: 1 })).title).toBe("");
    expect(repoOf(envelope<RepoEnvelope>({ description: "x" })).full_name).toBe("");
    const notification = notificationOf(envelope<NotificationInboxItem>({ id: 1 }));
    expect(notification.id).toBe("");
    expect(notification.subject.title).toBe("");
    expect(asGithubPull("garbage").number).toBe(0);
  });
});
