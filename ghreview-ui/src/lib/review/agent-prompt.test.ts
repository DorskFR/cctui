import { describe, expect, it } from "vitest";
import type { GithubPull } from "../api/types";
import { buildReviewPrompt, headSha, pullUrl } from "./agent-prompt";

function pull(overrides: Partial<GithubPull> = {}): GithubPull {
  return {
    number: 46,
    title: "Add the review plugin",
    state: "open",
    changed_files: 12,
    head: { ref: "lane/p3", sha: "1111111111111111111111111111111111111111" },
    base: { ref: "main", sha: "2222222222222222222222222222222222222222" },
    html_url: "https://github.com/DorskFR/cctui/pull/46",
    ...overrides,
  } as GithubPull;
}

describe("pullUrl", () => {
  it("prefers the synced html_url", () => {
    expect(pullUrl("DorskFR", "cctui", pull())).toBe("https://github.com/DorskFR/cctui/pull/46");
  });

  it("falls back to the canonical GitHub path", () => {
    expect(pullUrl("DorskFR", "cctui", pull({ html_url: undefined }))).toBe(
      "https://github.com/DorskFR/cctui/pull/46",
    );
  });
});

describe("headSha", () => {
  it("prefers the enriched head sha over the payload's", () => {
    expect(headSha(pull({ cctui_enriched_head_sha: "abc123" }))).toBe("abc123");
  });

  it("falls back to the head ref's sha", () => {
    expect(headSha(pull())).toBe("1111111111111111111111111111111111111111");
  });

  it("is null when the payload has no head", () => {
    expect(headSha(pull({ head: undefined }))).toBeNull();
  });
});

describe("buildReviewPrompt", () => {
  it("names the repo, number, title, url, head sha and base", () => {
    const prompt = buildReviewPrompt({ owner: "DorskFR", repo: "cctui", pull: pull() });

    expect(prompt).toContain("DorskFR/cctui#46");
    expect(prompt).toContain("Add the review plugin");
    expect(prompt).toContain("https://github.com/DorskFR/cctui/pull/46");
    expect(prompt).toContain("lane/p3 at 1111111111111111111111111111111111111111");
    expect(prompt).toContain("Base: main");
    expect(prompt).toContain("Changed files: 12");
  });

  it("pins the review to the head sha so a moving branch cannot change the target", () => {
    const prompt = buildReviewPrompt({
      owner: "o",
      repo: "r",
      pull: pull({ cctui_enriched_head_sha: "deadbeef" }),
    });

    expect(prompt).toContain("Review the diff at head SHA deadbeef");
  });

  it("references the gh-review skill by default", () => {
    expect(buildReviewPrompt({ owner: "o", repo: "r", pull: pull() })).toContain(
      "gh-review skill",
    );
  });

  it("omits the skill line when the plugin ships no skill", () => {
    const prompt = buildReviewPrompt({ owner: "o", repo: "r", pull: pull(), skill: null });

    expect(prompt).not.toContain("skill");
    expect(prompt).toContain("Review the diff");
  });

  it("degrades gracefully when the payload is missing refs and counts", () => {
    const prompt = buildReviewPrompt({
      owner: "o",
      repo: "r",
      pull: pull({ head: undefined, base: undefined, changed_files: undefined }),
    });

    expect(prompt).toContain("Head: unknown");
    expect(prompt).toContain("Base: unknown");
    expect(prompt).not.toContain("Changed files");
    expect(prompt).toContain("Review the diff at the head of the pull request");
  });
});
