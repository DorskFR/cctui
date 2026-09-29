// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("$app/environment", () => ({ browser: true }));

const LEGACY_KEY = "cctui_theme_pref";
const KIT_KEY = "tsumikit-theme";

async function boot() {
  vi.resetModules();
  return await import("./theme.svelte");
}

beforeEach(() => localStorage.clear());
afterEach(() => localStorage.clear());

describe("legacy cctui_theme_pref migration", () => {
  it("adopts the legacy preference, persists it under the kit key and drops it", async () => {
    localStorage.setItem(
      LEGACY_KEY,
      JSON.stringify({ mode: "auto", light: "sepia", dark: "mocha" }),
    );

    const { theme } = await boot();

    expect(theme.pref).toEqual({ mode: "auto", light: "sepia", dark: "mocha" });
    expect(theme.choice).toBe("auto");
    expect(localStorage.getItem(LEGACY_KEY)).toBeNull();
    expect(JSON.parse(localStorage.getItem(KIT_KEY) ?? "{}")).toEqual({
      mode: "auto",
      light: "sepia",
      dark: "mocha",
    });
  });

  it("keeps a pinned legacy choice and both remembered slots", async () => {
    localStorage.setItem(
      LEGACY_KEY,
      JSON.stringify({ mode: "light", light: "latte", dark: "dracula" }),
    );

    const { theme } = await boot();

    expect(theme.choice).toBe("latte");
    expect(theme.resolved).toBe("latte");
    expect(theme.pref.dark).toBe("dracula");
  });

  it("falls back to the kit's own value when the legacy blob is junk", async () => {
    localStorage.setItem(KIT_KEY, "nord");
    localStorage.setItem(LEGACY_KEY, "{not json");

    const { theme } = await boot();

    expect(theme.resolved).toBe("nord");
    expect(localStorage.getItem(LEGACY_KEY)).toBe("{not json");
  });

  it("leaves the kit store alone when there is nothing to migrate", async () => {
    localStorage.setItem(
      KIT_KEY,
      JSON.stringify({ mode: "dark", light: "light", dark: "gruvbox" }),
    );

    const { theme } = await boot();

    expect(theme.resolved).toBe("gruvbox");
  });
});

describe("theme choices mirror into the settings blob", () => {
  it("picking a theme records the preference and the resolved id", async () => {
    const { theme } = await boot();
    const { auth } = await import("./auth.svelte");
    const { settings } = await import("./settings.svelte");
    auth.isAuthed = false;

    theme.choose("sepia");
    expect(settings.state.display.theme).toBe("sepia");
    expect(settings.state.display.themeMode).toBe("light");
    expect(settings.state.display.lightTheme).toBe("sepia");

    theme.choose("mocha");
    theme.choose("auto");
    expect(settings.state.display.themeMode).toBe("auto");
    expect(settings.state.display.lightTheme).toBe("sepia");
    expect(settings.state.display.darkTheme).toBe("mocha");
  });
});
