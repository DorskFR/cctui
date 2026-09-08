import { describe, expect, it } from "vitest";
import { isValidAccountEmoji } from "./avatar";
import { EMOJI_GROUPS, searchEmoji } from "./emojiCatalog";

describe("emoji catalogue", () => {
  it("every entry is a valid single account emoji", () => {
    for (const g of EMOJI_GROUPS)
      for (const x of g.entries) expect(isValidAccountEmoji(x.emoji), x.emoji).toBe(true);
  });
  it("has no duplicate emoji", () => {
    const all = EMOJI_GROUPS.flatMap((g) => g.entries.map((x) => x.emoji));
    expect(new Set(all).size).toBe(all.length);
  });
});

describe("searchEmoji", () => {
  it("is empty for a blank query", () => {
    expect(searchEmoji("  ")).toEqual([]);
  });
  it("matches keywords in both languages, accent-insensitively", () => {
    expect(searchEmoji("crabe").map((x) => x.emoji)).toEqual(["🦀"]);
    expect(searchEmoji("elephant").map((x) => x.emoji)).toEqual(["🐘"]);
    expect(searchEmoji("Éléphant").map((x) => x.emoji)).toEqual(["🐘"]);
  });
  it("requires every word to match", () => {
    expect(searchEmoji("heart blue").map((x) => x.emoji)).toEqual(["💙"]);
  });
});
