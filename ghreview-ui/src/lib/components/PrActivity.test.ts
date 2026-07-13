import { describe, expect, it } from "vitest";
import type { ActivityEvent } from "../api/types";
import { iconOf, kindOf, phraseOf, relativeTime } from "./PrActivity.svelte";

function ev(partial: Partial<ActivityEvent> & { event: string }): ActivityEvent {
  return { actor: null, created_at: null, ...partial } as ActivityEvent;
}

describe("phraseOf", () => {
  it("phrases review verdicts by state", () => {
    expect(phraseOf(ev({ event: "reviewed", detail: { state: "APPROVED" } }))).toBe(
      "approved these changes",
    );
    expect(phraseOf(ev({ event: "reviewed", detail: { state: "CHANGES_REQUESTED" } }))).toBe(
      "requested changes",
    );
    expect(phraseOf(ev({ event: "reviewed", detail: {} }))).toBe("reviewed");
  });

  it("names labels and reviewers", () => {
    expect(
      phraseOf(ev({ event: "labeled", detail: { label: { name: "bug", color: null } } })),
    ).toBe("added the bug label");
    expect(
      phraseOf(
        ev({ event: "review_requested", detail: { reviewer: { login: "erin", avatar_url: null } } }),
      ),
    ).toBe("requested a review from erin");
  });

  it("quotes rename titles", () => {
    expect(phraseOf(ev({ event: "renamed", detail: { from: "old", to: "new" } }))).toBe(
      "renamed this from “old” to “new”",
    );
  });

  it("handles merge/close/reopen", () => {
    expect(phraseOf(ev({ event: "merged" }))).toBe("merged this pull request");
    expect(phraseOf(ev({ event: "closed" }))).toBe("closed this pull request");
    expect(phraseOf(ev({ event: "reopened" }))).toBe("reopened this pull request");
  });
});

describe("kindOf / iconOf", () => {
  it("colors reviews by state", () => {
    expect(kindOf(ev({ event: "reviewed", detail: { state: "APPROVED" } }))).toBe("approved");
    expect(kindOf(ev({ event: "reviewed", detail: { state: "CHANGES_REQUESTED" } }))).toBe(
      "changes",
    );
    expect(kindOf(ev({ event: "committed" }))).toBe("commit");
  });

  it("returns a non-empty svg path for every event", () => {
    for (const e of ["committed", "reviewed", "merged", "renamed", "labeled", "assigned"]) {
      expect(iconOf(ev({ event: e })).length).toBeGreaterThan(0);
    }
  });
});

describe("relativeTime", () => {
  it("returns empty for null or invalid input", () => {
    expect(relativeTime(null)).toBe("");
    expect(relativeTime("not-a-date")).toBe("");
  });

  it("formats a recent past timestamp", () => {
    const iso = new Date(Date.now() - 5 * 60_000).toISOString();
    expect(relativeTime(iso)).toContain("minute");
  });
});
