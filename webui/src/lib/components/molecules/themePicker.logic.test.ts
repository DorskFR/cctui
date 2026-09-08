import { describe, expect, it } from "vitest";
import {
  autoLabel,
  themePickerGroups,
  themePickerTitle,
} from "./themePicker.logic";

const all = [
  { id: "light", label: "Light", mode: "light" as const, icon: "☀" },
  { id: "sepia", label: "Sepia", mode: "light" as const, icon: "✶" },
  { id: "dark", label: "Dark", mode: "dark" as const, icon: "☾" },
  { id: "mocha", label: "Catppuccin Mocha", mode: "dark" as const },
];
const pref = { mode: "auto" as const, light: "sepia", dark: "mocha" };
const labels = { auto: "Auto", light: "Light", dark: "Dark" };

describe("themePickerGroups", () => {
  it("puts Auto first, then the light and dark sections", () => {
    const g = themePickerGroups(all, pref, labels);
    expect(g.map((x) => x.label)).toEqual(["Auto", "Light", "Dark"]);
    expect(g[0].options).toEqual([
      { value: "auto", label: "◐  Auto · ✶ Sepia / ◈ Catppuccin Mocha" },
    ]);
    expect(g[1].options.map((o) => o.value)).toEqual(["light", "sepia"]);
    expect(g[2].options.map((o) => o.value)).toEqual(["dark", "mocha"]);
  });
});

describe("autoLabel / themePickerTitle", () => {
  it("recalls both remembered themes", () => {
    expect(autoLabel(all, pref, "Auto")).toBe(
      "Auto · ✶ Sepia / ◈ Catppuccin Mocha",
    );
  });
  it("the title names the painted theme, prefixed by Auto when it follows the system", () => {
    expect(themePickerTitle(all, pref, "mocha", "Auto")).toBe(
      "Auto · Catppuccin Mocha",
    );
    expect(themePickerTitle(all, { ...pref, mode: "light" }, "sepia", "Auto")).toBe("Sepia");
  });
});
