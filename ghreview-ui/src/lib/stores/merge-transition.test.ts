import { describe, expect, it } from "vitest";
import { closeMergedPr } from "./merge-transition";
import { defaultStatus, openTab, type PrTab, type TabsState } from "./tabs-core";

function tab(owner: string, repo: string, number: number): PrTab {
  return {
    id: `pr-${owner}-${repo}-${number}`,
    owner,
    repo,
    number,
    title: `Pull request ${number}`,
    status: defaultStatus(),
  };
}

describe("merged pull request transition", () => {
  it("closes the active tab and routes to the adjacent remaining pull request", () => {
    let state: TabsState = { tabs: [], activeId: null };
    state = openTab(state, tab("example", "project", 10));
    state = openTab(state, tab("example", "project", 20));
    state = openTab(state, tab("example", "project", 30));

    const transition = closeMergedPr(state, "example", "project", 20);

    expect(transition.state.tabs.map((item) => item.number)).toEqual([10, 30]);
    expect(transition.state.activeId).toBe("pr-example-project-30");
    expect(transition.path).toBe("/example/project/pull/30");
  });

  it("routes to the pull request list after closing the final tab", () => {
    const state = openTab({ tabs: [], activeId: null }, tab("example", "project", 10));

    const transition = closeMergedPr(state, "example", "project", 10);

    expect(transition.state).toEqual({ tabs: [], activeId: null });
    expect(transition.path).toBe("/");
  });
});
