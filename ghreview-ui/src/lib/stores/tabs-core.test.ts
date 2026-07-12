import { describe, expect, it } from "vitest";
import {
  closeTab,
  defaultStatus,
  deserialize,
  openTab,
  type PrTab,
  serialize,
  setActive,
  tabId,
  type TabsState,
  updateStatus,
} from "./tabs-core";

function tab(owner: string, repo: string, n: number): PrTab {
  return {
    id: tabId(owner, repo, n),
    owner,
    repo,
    number: n,
    title: `${repo} #${n}`,
    status: defaultStatus(),
  };
}

const empty: TabsState = { tabs: [], activeId: null };

describe("tab ids", () => {
  it("are deterministic from coordinates", () => {
    expect(tabId("DorskFR", "cctui", 12)).toBe("pr-DorskFR-cctui-12");
  });
});

describe("openTab", () => {
  it("adds and activates a new tab", () => {
    const s = openTab(empty, tab("o", "r", 1));
    expect(s.tabs).toHaveLength(1);
    expect(s.activeId).toBe("pr-o-r-1");
  });

  it("is idempotent — reopening re-activates without duplicating", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = openTab(s, tab("o", "r", 2));
    s = openTab(s, tab("o", "r", 1));
    expect(s.tabs).toHaveLength(2);
    expect(s.activeId).toBe("pr-o-r-1");
  });

  it("preserves live status when reopening", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = updateStatus(s, "pr-o-r-1", { ci: "success" });
    s = openTab(s, tab("o", "r", 1));
    expect(s.tabs[0].status.ci).toBe("success");
  });

  it("returns the same reference when reopening the already-active tab unchanged", () => {
    const s = openTab(empty, tab("o", "r", 1));
    expect(openTab(s, tab("o", "r", 1))).toBe(s);
  });
});

describe("reference stability (guards the reactive tabs effect from self-looping)", () => {
  it("setActive returns the same reference when already active", () => {
    const s = openTab(empty, tab("o", "r", 1));
    expect(setActive(s, "pr-o-r-1")).toBe(s);
  });

  it("updateStatus returns the same reference when status is unchanged", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = updateStatus(s, "pr-o-r-1", { ci: "success" });
    expect(updateStatus(s, "pr-o-r-1", { ci: "success" })).toBe(s);
  });

  it("updateStatus still produces a new reference on a real change", () => {
    const s = openTab(empty, tab("o", "r", 1));
    expect(updateStatus(s, "pr-o-r-1", { ci: "success" })).not.toBe(s);
  });
});

describe("closeTab", () => {
  it("selects the adjacent tab when closing the active one", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = openTab(s, tab("o", "r", 2));
    s = openTab(s, tab("o", "r", 3));
    s = setActive(s, "pr-o-r-2");
    s = closeTab(s, "pr-o-r-2");
    expect(s.activeId).toBe("pr-o-r-3");
    expect(s.tabs.map((t) => t.number)).toEqual([1, 3]);
  });

  it("clears active when closing the last tab", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = closeTab(s, "pr-o-r-1");
    expect(s).toEqual(empty);
  });

  it("leaves active untouched when closing an inactive tab", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = openTab(s, tab("o", "r", 2));
    s = closeTab(s, "pr-o-r-1");
    expect(s.activeId).toBe("pr-o-r-2");
  });
});

describe("persistence round-trip", () => {
  it("serialize then deserialize restores tabs and active id", () => {
    let s = openTab(empty, tab("o", "r", 1));
    s = openTab(s, tab("o", "r", 2));
    const restored = deserialize(serialize(s));
    expect(restored).toEqual(s);
  });

  it("drops malformed tabs and falls back to last tab for active", () => {
    const raw = JSON.stringify({
      tabs: [
        { id: "pr-o-r-1", owner: "o", repo: "r", number: 1, title: "x", status: defaultStatus() },
        { id: "bad" },
      ],
      activeId: "missing",
    });
    const s = deserialize(raw);
    expect(s.tabs).toHaveLength(1);
    expect(s.activeId).toBe("pr-o-r-1");
  });

  it("returns empty on garbage input", () => {
    expect(deserialize("not json")).toEqual(empty);
    expect(deserialize(null)).toEqual(empty);
  });
});
