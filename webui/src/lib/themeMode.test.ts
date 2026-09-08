import { describe, expect, it } from "vitest";
import {
  AUTO_THEME,
  chooseTheme,
  DEFAULT_THEME_PREFERENCE,
  pickerValue,
  preferenceFrom,
  resolveTheme,
  type ThemePreference,
} from "./themeMode";

const LIGHT = new Set(["light", "sepia", "latte"]);
const DARK = new Set(["dark", "mocha", "nord"]);
const slotOf = (id: string) =>
  LIGHT.has(id) ? ("light" as const) : DARK.has(id) ? ("dark" as const) : null;

const pref: ThemePreference = { mode: "auto", light: "sepia", dark: "mocha" };

describe("resolveTheme", () => {
  it("auto follows the system scheme", () => {
    expect(resolveTheme(pref, true)).toBe("mocha");
    expect(resolveTheme(pref, false)).toBe("sepia");
  });
  it("a pinned mode ignores the system", () => {
    expect(resolveTheme({ ...pref, mode: "light" }, true)).toBe("sepia");
    expect(resolveTheme({ ...pref, mode: "dark" }, false)).toBe("mocha");
  });
});

describe("chooseTheme", () => {
  it("a light theme pins light and becomes the remembered light", () => {
    expect(chooseTheme(pref, "latte", slotOf)).toEqual({
      mode: "light",
      light: "latte",
      dark: "mocha",
    });
  });
  it("a dark theme pins dark and keeps the light memory", () => {
    expect(chooseTheme(pref, "nord", slotOf)).toEqual({
      mode: "dark",
      light: "sepia",
      dark: "nord",
    });
  });
  it("auto switches the mode and keeps both memories", () => {
    const pinned = { ...pref, mode: "dark" as const };
    expect(chooseTheme(pinned, AUTO_THEME, slotOf)).toEqual(pref);
  });
  it("an unknown id is ignored", () => {
    expect(chooseTheme(pref, "nope", slotOf)).toBe(pref);
  });
});

describe("preferenceFrom", () => {
  it("reads the three new fields", () => {
    expect(
      preferenceFrom(
        { themeMode: "auto", lightTheme: "latte", darkTheme: "nord" },
        slotOf,
      ),
    ).toEqual({ mode: "auto", light: "latte", dark: "nord" });
  });
  it("a legacy blob with a lone theme pins its slot", () => {
    expect(preferenceFrom({ theme: "sepia" }, slotOf)).toEqual({
      mode: "light",
      light: "sepia",
      dark: "dark",
    });
    expect(preferenceFrom({ theme: "nord" }, slotOf)).toEqual({
      mode: "dark",
      light: "light",
      dark: "nord",
    });
  });
  it("slot memories of the wrong kind or unknown fall back to defaults", () => {
    expect(
      preferenceFrom(
        { themeMode: "light", lightTheme: "mocha", darkTheme: "gone" },
        slotOf,
      ),
    ).toEqual({ mode: "light", light: "light", dark: "dark" });
  });
  it("an empty blob is the default", () => {
    expect(preferenceFrom({}, slotOf)).toEqual(DEFAULT_THEME_PREFERENCE);
  });
});

describe("pickerValue", () => {
  it("is auto in auto mode, else the pinned slot's theme", () => {
    expect(pickerValue(pref)).toBe(AUTO_THEME);
    expect(pickerValue({ ...pref, mode: "light" })).toBe("sepia");
    expect(pickerValue({ ...pref, mode: "dark" })).toBe("mocha");
  });
});
