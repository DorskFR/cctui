import { describe, expect, it } from "vitest";
import {
  defaultLayout,
  deserialize,
  type LayoutState,
  serialize,
  setSidebarCollapsed,
  toggleSidebar,
} from "./layout-core";

describe("layout defaults", () => {
  it("defaults to an expanded sidebar", () => {
    expect(defaultLayout()).toEqual({ sidebarCollapsed: false });
  });
});

describe("sidebar collapse", () => {
  it("toggles the collapsed flag", () => {
    const a = defaultLayout();
    expect(toggleSidebar(a).sidebarCollapsed).toBe(true);
    expect(toggleSidebar(toggleSidebar(a)).sidebarCollapsed).toBe(false);
  });

  it("sets an explicit collapsed value", () => {
    expect(setSidebarCollapsed(defaultLayout(), true).sidebarCollapsed).toBe(true);
    const collapsed: LayoutState = { sidebarCollapsed: true };
    expect(setSidebarCollapsed(collapsed, false).sidebarCollapsed).toBe(false);
  });
});

describe("serialization", () => {
  it("round-trips", () => {
    const state: LayoutState = { sidebarCollapsed: true };
    expect(deserialize(serialize(state))).toEqual(state);
  });

  it("falls back to defaults on null or garbage", () => {
    expect(deserialize(null)).toEqual(defaultLayout());
    expect(deserialize("{not json")).toEqual(defaultLayout());
    expect(deserialize("{}")).toEqual(defaultLayout());
  });
});
