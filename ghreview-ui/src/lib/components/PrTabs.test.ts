import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { PrContentTab } from "../stores/pr-tabs-core";
import PrTabs from "./PrTabs.svelte";

let component: ReturnType<typeof mount> | undefined;

function render(active: PrContentTab, onselect: (tab: PrContentTab) => void) {
  component = mount(PrTabs, {
    target: document.body,
    props: { active, counts: { diff: 7 }, onselect },
  });
}

function tablist(): HTMLElement {
  return document.querySelector('[role="tablist"]') as HTMLElement;
}

function tabLabels(): string[] {
  return [...document.querySelectorAll('[role="tab"]')].map((t) =>
    (t.textContent ?? "").trim().replace(/\s+/g, " "),
  );
}

function press(key: string): void {
  tablist().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
}

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("PrTabs", () => {
  it("renders a labelled tablist of the PR content tabs with counts", async () => {
    render("diff", vi.fn());
    await tick();

    expect(tablist().getAttribute("aria-label")).toBe("Pull request content");
    expect(tabLabels()).toEqual(["Description", "Commits", "Comments", "Diff 7"]);

    const selected = document.querySelector('[role="tab"][aria-selected="true"]');
    expect((selected?.textContent ?? "").trim()).toContain("Diff");
  });

  it("keeps exactly one tab in the Tab order (roving tabindex)", async () => {
    render("commits", vi.fn());
    await tick();

    const tabindexes = [...document.querySelectorAll('[role="tab"]')].map((t) =>
      t.getAttribute("tabindex"),
    );
    expect(tabindexes).toEqual(["-1", "0", "-1", "-1"]);
  });

  it("selects the previous and next tab with the arrow keys", async () => {
    const onselect = vi.fn<(tab: PrContentTab) => void>();
    render("comments", onselect);
    await tick();

    press("ArrowRight");
    expect(onselect).toHaveBeenLastCalledWith("diff");

    press("ArrowLeft");
    expect(onselect).toHaveBeenLastCalledWith("commits");
  });

  it("wraps around and jumps to the ends with Home and End", async () => {
    const onselect = vi.fn<(tab: PrContentTab) => void>();
    render("diff", onselect);
    await tick();

    press("ArrowRight");
    expect(onselect).toHaveBeenLastCalledWith("description");

    press("Home");
    expect(onselect).toHaveBeenLastCalledWith("description");

    press("End");
    expect(onselect).toHaveBeenLastCalledWith("diff");
  });

  it("selects a tab on click", async () => {
    const onselect = vi.fn<(tab: PrContentTab) => void>();
    render("diff", onselect);
    await tick();

    const commits = [...document.querySelectorAll('[role="tab"]')].find((t) =>
      (t.textContent ?? "").includes("Commits"),
    ) as HTMLElement;
    commits.click();

    expect(onselect).toHaveBeenCalledWith("commits");
  });
});
