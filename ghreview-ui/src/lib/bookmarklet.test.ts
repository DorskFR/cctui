import { describe, expect, it } from "vitest";
import { bookmarkletSource, rewriteGithubUrl } from "./bookmarklet";

describe("rewriteGithubUrl", () => {
  it("mirrors a github PR url onto this host", () => {
    expect(
      rewriteGithubUrl("https://github.com/DorskFR/cctui/pull/42", "https://review.example.com"),
    ).toBe("https://review.example.com/DorskFR/cctui/pull/42");
  });

  it("tolerates trailing path segments and a trailing slash on origin", () => {
    expect(
      rewriteGithubUrl("https://github.com/o/r/pull/9/files", "https://h/"),
    ).toBe("https://h/o/r/pull/9");
  });

  it("returns null for non-PR urls", () => {
    expect(rewriteGithubUrl("https://github.com/o/r/issues/1", "https://h")).toBeNull();
  });
});

describe("bookmarkletSource", () => {
  it("produces a javascript: snippet embedding the origin", () => {
    const src = bookmarkletSource("https://h");
    expect(src.startsWith("javascript:(function()")).toBe(true);
    expect(src).toContain('"https://h"');
    expect(src).toContain("github");
  });
});
