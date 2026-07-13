import { describe, expect, it } from "vitest";
import {
  defaultPrTab,
  deserializePrTab,
  isPrContentTab,
  PR_CONTENT_TABS,
  prTabStorageKey,
} from "./pr-tabs-core";

describe("pr content tabs", () => {
  it("defaults to the diff tab", () => {
    expect(defaultPrTab()).toBe("diff");
  });

  it("exposes the content tabs in order", () => {
    expect(PR_CONTENT_TABS).toEqual([
      "description",
      "conversation",
      "commits",
      "checks",
      "activity",
      "diff",
    ]);
  });

  it("recognizes valid tab ids", () => {
    expect(isPrContentTab("commits")).toBe(true);
    expect(isPrContentTab("nope")).toBe(false);
    expect(isPrContentTab(null)).toBe(false);
  });

  it("builds a per-PR storage key", () => {
    expect(prTabStorageKey("acme", "web", 42)).toBe("ghreview:prtab:acme/web/42");
  });

  it("deserializes to a valid tab or the default", () => {
    expect(deserializePrTab("checks")).toBe("checks");
    expect(deserializePrTab(null)).toBe("diff");
    expect(deserializePrTab("garbage")).toBe("diff");
  });
});
