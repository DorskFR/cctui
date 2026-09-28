import { describe, expect, it } from "vitest";
import { parseScrubPatterns } from "./scrub-patterns";

describe("parseScrubPatterns", () => {
  it("keeps one entry per non-empty line", () => {
    const { patterns, issues } = parseScrubPatterns("ACME-[0-9]{6}\n\n  MYCORP_\\w+  \n");
    expect(issues).toEqual([]);
    expect(patterns).toEqual([
      { name: "custom", regex: "ACME-[0-9]{6}", enabled: true },
      { name: "custom", regex: "MYCORP_\\w+", enabled: true },
    ]);
  });

  it("drops duplicates, as the server does", () => {
    const { patterns } = parseScrubPatterns("a\\d+\na\\d+");
    expect(patterns).toHaveLength(1);
  });

  it("reports a glob-style line by number and keeps it out of the payload", () => {
    const { patterns, issues } = parseScrubPatterns("ACME-[0-9]{6}\n*_token\nMYCORP_\\w+");
    expect(patterns.map((p) => p.regex)).toEqual(["ACME-[0-9]{6}", "MYCORP_\\w+"]);
    expect(issues).toHaveLength(1);
    expect(issues[0].line).toBe(2);
    expect(issues[0].regex).toBe("*_token");
    expect(issues[0].message).not.toBe("");
  });

  it("has nothing to save and nothing to complain about when empty", () => {
    expect(parseScrubPatterns("\n  \n")).toEqual({ patterns: [], issues: [] });
  });
});
