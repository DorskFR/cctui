import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ReactionContent, ReactionSummary } from "../api/types";
import ReactionBar from "./ReactionBar.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function emptySummary(overrides: Partial<ReactionSummary> = {}): ReactionSummary {
  return {
    "+1": 0,
    "-1": 0,
    laugh: 0,
    hooray: 0,
    confused: 0,
    heart: 0,
    rocket: 0,
    eyes: 0,
    total_count: 0,
    viewer_reactions: [],
    ...overrides,
  };
}

describe("ReactionBar", () => {
  it("renders only non-zero pills and hides zero-count reactions", () => {
    component = mount(ReactionBar, {
      target: document.body,
      props: {
        reactions: { "+1": 3, heart: 1, total_count: 4 },
        viewerReactions: ["+1"],
        onToggle: async () => emptySummary(),
      },
    });
    const pills = document.querySelectorAll(".pill");
    expect(pills.length).toBe(2);
    expect(document.body.textContent).toContain("👍");
    expect(document.body.textContent).toContain("3");
    // A zero-count reaction like 🎉 must not appear as a pill.
    const pillText = [...pills].map((p) => p.textContent ?? "").join("");
    expect(pillText).not.toContain("🎉");
    expect(document.querySelector(".add")).not.toBeNull();
  });

  it("highlights the viewer's own reaction", () => {
    component = mount(ReactionBar, {
      target: document.body,
      props: {
        reactions: { "+1": 1, total_count: 1 },
        viewerReactions: ["+1"],
        onToggle: async () => emptySummary(),
      },
    });
    const mine = document.querySelector(".pill.mine");
    expect(mine).not.toBeNull();
    expect(mine?.getAttribute("aria-pressed")).toBe("true");
  });

  it("toggles and reconciles counts from the summary returned by onToggle", async () => {
    const onToggle = vi.fn(
      async (content: ReactionContent): Promise<ReactionSummary> =>
        emptySummary({ [content]: 2, total_count: 2, viewer_reactions: [content] }),
    );
    component = mount(ReactionBar, {
      target: document.body,
      props: { reactions: { "+1": 1, total_count: 1 }, viewerReactions: [], onToggle },
    });
    const pill = document.querySelector(".pill") as HTMLButtonElement;
    pill.click();
    await tick();
    await tick();
    expect(onToggle).toHaveBeenCalledWith("+1");
    expect(document.querySelector(".pill")?.textContent).toContain("2");
    expect(document.querySelector(".pill.mine")).not.toBeNull();
  });

  it("opens the add-menu exposing the full reaction set", async () => {
    component = mount(ReactionBar, {
      target: document.body,
      props: { reactions: null, viewerReactions: [], onToggle: async () => emptySummary() },
    });
    expect(document.querySelector(".menu")).toBeNull();
    const add = document.querySelector(".add") as HTMLButtonElement;
    add.click();
    await tick();
    const opts = document.querySelectorAll(".menu .opt");
    expect(opts.length).toBe(8);
  });
});
