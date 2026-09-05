import { describe, expect, it } from "vitest";
import type { SessionListItem } from "@bindings/SessionListItem";
import {
  applyMention,
  filterMentions,
  findTrigger,
  mentionToken,
  mentionableSessions,
  moveSelection,
} from "./mention";

const sess = (p: Partial<SessionListItem>): SessionListItem =>
  ({
    id: "id",
    status: "active",
    bucket: "working",
    working_dir: "/w",
    name: null,
    machine_name: "agents",
    ...p,
  }) as SessionListItem;

describe("findTrigger", () => {
  it("opens on a bare # at the caret", () => {
    expect(findTrigger("sync with #", 11)).toEqual({ start: 10, query: "" });
  });
  it("carries the typed query", () => {
    expect(findTrigger("sync with #gree", 15)).toEqual({
      start: 10,
      query: "gree",
    });
  });
  it("closes once whitespace follows the #", () => {
    expect(findTrigger("ticket #12 ", 11)).toBeNull();
    expect(findTrigger("ticket #12 next", 15)).toBeNull();
  });
  it("ignores a # glued to a word (C#, url fragment)", () => {
    expect(findTrigger("C#", 2)).toBeNull();
    expect(findTrigger("a.com/x#y", 9)).toBeNull();
  });
  it("only looks before the caret", () => {
    expect(findTrigger("#abc", 0)).toBeNull();
    expect(findTrigger("#abc tail", 4)).toEqual({ start: 0, query: "abc" });
  });
});

describe("mentionableSessions", () => {
  it("keeps every bucket, done included, and drops archived, draft and self", () => {
    const list = [
      sess({ id: "w", bucket: "working" }),
      sess({ id: "b", bucket: "blocked" }),
      sess({ id: "r", bucket: "review" }),
      sess({ id: "d", bucket: "done" }),
      sess({ id: "a", status: "archived" }),
      sess({ id: "dr", status: "draft" }),
      sess({ id: "me" }),
    ];
    expect(mentionableSessions(list, "me").map((s) => s.id)).toEqual([
      "w",
      "b",
      "r",
      "d",
    ]);
    expect(mentionableSessions(list).map((s) => s.id)).toEqual([
      "w",
      "b",
      "r",
      "d",
      "me",
    ]);
  });
});

describe("filterMentions", () => {
  const list = [
    sess({
      id: "01a0-1",
      name: "Examiner le projet greenfield",
      working_dir: "/home/x/green",
    }),
    sess({ id: "02b0-2", name: "Fix login", machine_name: "vps" }),
  ];
  it("returns all on an empty query", () => {
    expect(filterMentions(list, "")).toHaveLength(2);
  });
  it("matches name, id, dir and machine case-insensitively", () => {
    expect(filterMentions(list, "GREEN").map((s) => s.id)).toEqual(["01a0-1"]);
    expect(filterMentions(list, "02b0").map((s) => s.id)).toEqual(["02b0-2"]);
    expect(filterMentions(list, "vps").map((s) => s.id)).toEqual(["02b0-2"]);
    expect(filterMentions(list, "nope")).toEqual([]);
  });
});

describe("mentionToken / applyMention", () => {
  it("formats #<id> (<name>) and falls back to #<id>", () => {
    expect(mentionToken({ id: "x", name: "Greenfield" })).toBe(
      "#x (Greenfield)",
    );
    expect(mentionToken({ id: "x", name: "  " })).toBe("#x");
    expect(mentionToken({ id: "x", name: null })).toBe("#x");
  });
  it("replaces the #query and places the caret after the trailing space", () => {
    const text = "sync with #gre and go";
    const trig = findTrigger(text, 14)!;
    const out = applyMention(text, 14, trig, { id: "ID1", name: "Green" });
    expect(out.text).toBe("sync with #ID1 (Green)  and go");
    expect(out.caret).toBe("sync with #ID1 (Green) ".length);
  });
});

describe("moveSelection", () => {
  it("wraps both ways and tolerates an empty list", () => {
    expect(moveSelection(0, 1, 3)).toBe(1);
    expect(moveSelection(2, 1, 3)).toBe(0);
    expect(moveSelection(0, -1, 3)).toBe(2);
    expect(moveSelection(0, 1, 0)).toBe(0);
  });
});
