import { describe, expect, it } from "vitest";
import {
  defaultLayout,
  deserialize,
  type LayoutState,
  serialize,
  setMode,
  toggleFullWidth,
  toggleMode,
} from "./layout-core";

describe("layout defaults", () => {
  it("defaults to master-detail panels, not full width", () => {
    expect(defaultLayout()).toEqual({ mode: "panels", fullWidth: false });
  });
});

describe("mode", () => {
  it("toggles between panels and tabs", () => {
    const a = defaultLayout();
    const b = toggleMode(a);
    expect(b.mode).toBe("tabs");
    expect(toggleMode(b).mode).toBe("panels");
  });

  it("preserves fullWidth across mode toggles", () => {
    const state: LayoutState = { mode: "panels", fullWidth: true };
    expect(toggleMode(state).fullWidth).toBe(true);
  });

  it("sets an explicit mode", () => {
    expect(setMode(defaultLayout(), "tabs").mode).toBe("tabs");
  });
});

describe("fullWidth", () => {
  it("toggles the master-pane collapse flag", () => {
    const a = defaultLayout();
    expect(toggleFullWidth(a).fullWidth).toBe(true);
    expect(toggleFullWidth(toggleFullWidth(a)).fullWidth).toBe(false);
  });
});

describe("serialization", () => {
  it("round-trips", () => {
    const state: LayoutState = { mode: "tabs", fullWidth: true };
    expect(deserialize(serialize(state))).toEqual(state);
  });

  it("falls back to defaults on null or garbage", () => {
    expect(deserialize(null)).toEqual(defaultLayout());
    expect(deserialize("{not json")).toEqual(defaultLayout());
    expect(deserialize("{}")).toEqual(defaultLayout());
  });
});
