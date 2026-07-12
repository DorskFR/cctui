import { describe, expect, it } from "vitest";
import { parsePullApiUrl, pullPath } from "./route";

describe("parsePullApiUrl", () => {
  it("parses a GitHub API PR url", () => {
    expect(parsePullApiUrl("https://api.github.com/repos/DorskFR/cctui/pulls/46")).toEqual({
      owner: "DorskFR",
      repo: "cctui",
      number: 46,
    });
  });

  it("ignores trailing path/query/hash", () => {
    expect(
      parsePullApiUrl("https://api.github.com/repos/octo/cat/pulls/7?foo=1#frag"),
    ).toEqual({ owner: "octo", repo: "cat", number: 7 });
  });

  it("returns null for issue/release subjects", () => {
    expect(parsePullApiUrl("https://api.github.com/repos/octo/cat/issues/9")).toBeNull();
    expect(parsePullApiUrl("https://api.github.com/repos/octo/cat/releases/12")).toBeNull();
  });

  it("returns null for empty/null urls", () => {
    expect(parsePullApiUrl(null)).toBeNull();
    expect(parsePullApiUrl(undefined)).toBeNull();
    expect(parsePullApiUrl("")).toBeNull();
  });

  it("round-trips into pullPath", () => {
    const p = parsePullApiUrl("https://api.github.com/repos/a/b/pulls/3");
    expect(p && pullPath(p.owner, p.repo, p.number)).toBe("/a/b/pull/3");
  });
});
