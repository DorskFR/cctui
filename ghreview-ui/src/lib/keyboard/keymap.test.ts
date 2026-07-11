import { describe, expect, it } from "vitest";
import { type KeyEventLike, resolveKey } from "./keymap";

function ev(over: Partial<KeyEventLike>): KeyEventLike {
  return { key: "", ctrlKey: false, metaKey: false, shiftKey: false, altKey: false, ...over };
}

const idle = { gPending: false };

describe("resolveKey", () => {
  it("maps j/k to hunk nav and J/K to file nav", () => {
    expect(resolveKey(ev({ key: "j" }), idle).action).toEqual({ type: "nextHunk" });
    expect(resolveKey(ev({ key: "k" }), idle).action).toEqual({ type: "prevHunk" });
    expect(resolveKey(ev({ key: "J" }), idle).action).toEqual({ type: "nextFile" });
    expect(resolveKey(ev({ key: "K" }), idle).action).toEqual({ type: "prevFile" });
  });

  it("implements the two-key gd sequence", () => {
    const first = resolveKey(ev({ key: "g" }), idle);
    expect(first.action).toBeNull();
    expect(first.state.gPending).toBe(true);
    expect(resolveKey(ev({ key: "d" }), first.state).action).toEqual({ type: "gotoDiff" });
  });

  it("resets a dangling g on an unrelated key", () => {
    const res = resolveKey(ev({ key: "x" }), { gPending: true });
    expect(res.action).toBeNull();
    expect(res.state.gPending).toBe(false);
  });

  it("maps cmd/ctrl shortcuts for tabs and palette", () => {
    expect(resolveKey(ev({ key: "w", metaKey: true }), idle).action).toEqual({ type: "closeTab" });
    expect(resolveKey(ev({ key: "3", ctrlKey: true }), idle).action).toEqual({
      type: "selectTab",
      index: 2,
    });
    expect(resolveKey(ev({ key: "k", metaKey: true }), idle).action).toEqual({
      type: "openPalette",
    });
  });

  it("ignores nav keys while typing in inputs", () => {
    const target = { tagName: "INPUT" } as unknown as EventTarget;
    expect(resolveKey(ev({ key: "j", target }), idle).action).toBeNull();
  });

  it("still allows cmd+w from an input", () => {
    const target = { tagName: "INPUT" } as unknown as EventTarget;
    expect(resolveKey(ev({ key: "w", metaKey: true, target }), idle).action).toEqual({
      type: "closeTab",
    });
  });
});
