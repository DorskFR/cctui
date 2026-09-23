import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  CURRENT_VERSION,
  clampNavPosition,
  clampSessionListWidth,
  mergeDefaults,
  sessionListWidthSize,
  settings,
} from "./settings.svelte";
import { auth } from "./auth.svelte";

const KEY = "cctui_settings";

function loadFromCache() {
  const raw = localStorage.getItem(KEY);
  return mergeDefaults(
    raw ? (JSON.parse(raw) as Record<string, unknown>) : null,
  );
}

beforeEach(() => {
  auth.isAuthed = false; // keep persist() cache-only (no server PUT)
  localStorage.clear();
});

afterEach(() => {
  localStorage.clear();
});

describe("Settings save → load round-trip through the blob", () => {
  it("theme / fontScale / notify / locale survive a persist then reload", () => {
    settings.setDisplay({
      theme: "sepia",
      fontScale: 1.25,
      notifyEnabled: true,
      notifySound: false,
    });
    settings.setLocale("fr");

    const loaded = loadFromCache();
    expect(loaded.display.theme).toBe("sepia");
    expect(loaded.display.fontScale).toBe(1.25);
    expect(loaded.display.notifyEnabled).toBe(true);
    expect(loaded.display.notifySound).toBe(false);
    expect(loaded.locale).toBe("fr");
  });

  it("a full serialize → mergeDefaults preserves the whole catalogue", () => {
    const saved = mergeDefaults({
      sessionList: { sort: "name", view: "card", density: "compact" },
      display: { theme: "light", fontScale: 0.9, archiveShortcut: false },
      harnessMode: "sdk",
      whipStopPhrases: {
        mode: "replace",
        phrases: ["stop now"],
        guidance: "go",
      },
      secretScrubEnabled: true,
      secretScrubPatterns: [{ name: "tok", regex: "sk-\\w+", enabled: true }],
      sessionEmojiPrefix: true,
      autoResumeOnConnectionLoss: true,
      macros: {
        enabled: true,
        items: [
          {
            id: "m1",
            title: "Nettoyage",
            prompt: "range le dépôt",
            adapter: "claude-code",
            machine_id: null,
            working_dir: "/srv/app",
            model: "opus",
            effort: "high",
            pool_id: "p1",
            permission_mode: "yolo",
            confirm: false,
          },
        ],
      },
      shortcutsEnabled: true,
      locale: "en",
    } as Record<string, unknown>);
    const reloaded = mergeDefaults(
      JSON.parse(JSON.stringify(saved)) as Record<string, unknown>,
    );
    expect(reloaded).toEqual(saved);
  });

  it("the nav position defaults to top, survives a reload, and clamps unknown values", () => {
    expect(mergeDefaults(null).display.nav).toBe("top");
    settings.setNav("bottom");
    expect(loadFromCache().display.nav).toBe("bottom");
    settings.setNav("top");
    expect(loadFromCache().display.nav).toBe("top");

    const merged = mergeDefaults({
      display: { nav: "sideways" },
    } as unknown as Record<string, unknown>);
    expect(merged.display.nav).toBe("top");
    expect(clampNavPosition(undefined)).toBe("top");
  });

  it("groupBy defaults to status and migrates the legacy none", () => {
    expect(mergeDefaults(null).sessionList.groupBy).toBe("status");
    const legacy = mergeDefaults({
      sessionList: { groupBy: "none" },
    } as unknown as Record<string, unknown>);
    expect(legacy.sessionList.groupBy).toBe("status");
    expect(
      mergeDefaults({ sessionList: { groupBy: "machine" } } as Record<
        string,
        unknown
      >).sessionList.groupBy,
    ).toBe("machine");
    settings.setSessionList({ groupBy: "label" });
    expect(loadFromCache().sessionList.groupBy).toBe("label");
  });

  it("the sort direction defaults to desc, survives a reload, and clamps unknown values", () => {
    expect(mergeDefaults(null).sessionList.sortDir).toBe("desc");
    settings.setSessionList({ sort: "name", sortDir: "asc" });
    expect(loadFromCache().sessionList.sort).toBe("name");
    expect(loadFromCache().sessionList.sortDir).toBe("asc");
    const merged = mergeDefaults({
      sessionList: { sort: "created", sortDir: "sideways" },
    } as unknown as Record<string, unknown>);
    expect(merged.sessionList.sortDir).toBe("desc");
  });

  it("a blob that predates sortDir merges with the default direction", () => {
    const loaded = mergeDefaults({
      sessionList: { sort: "created", view: "list" },
    } as Record<string, unknown>);
    expect(loaded.sessionList.sort).toBe("created");
    expect(loaded.sessionList.sortDir).toBe("desc");
  });

  it("the Completed archive-all button defaults on, survives a reload, and only false turns it off", () => {
    expect(mergeDefaults(null).display.archiveDoneButton).toBe(true);
    settings.setArchiveDoneButton(false);
    expect(loadFromCache().display.archiveDoneButton).toBe(false);
    settings.setArchiveDoneButton(true);
    expect(loadFromCache().display.archiveDoneButton).toBe(true);
    const merged = mergeDefaults({
      display: { archiveDoneButton: "no" },
    } as unknown as Record<string, unknown>);
    expect(merged.display.archiveDoneButton).toBe(true);
  });

  it("the role-tinted background defaults off, survives a reload, and only true turns it on", () => {
    expect(mergeDefaults(null).display.roleTintedBackground).toBe(false);
    settings.setRoleTintedBackground(true);
    expect(loadFromCache().display.roleTintedBackground).toBe(true);
    settings.setRoleTintedBackground(false);
    expect(loadFromCache().display.roleTintedBackground).toBe(false);
    const merged = mergeDefaults({
      display: { roleTintedBackground: "yes" },
    } as unknown as Record<string, unknown>);
    expect(merged.display.roleTintedBackground).toBe(false);
  });

  it("list width and account-name toggle survive a persist then reload", () => {
    settings.setSessionList({ width: "full", accountNames: true });

    const loaded = loadFromCache();
    expect(loaded.sessionList.width).toBe("full");
    expect(loaded.sessionList.accountNames).toBe(true);
  });

  it("an unknown stored width clamps to the default and account names default off", () => {
    const merged = mergeDefaults({
      sessionList: { width: "gigantic" },
    } as unknown as Record<string, unknown>);
    expect(merged.sessionList.width).toBe("default");
    expect(merged.sessionList.accountNames).toBe(false);
    expect(clampSessionListWidth(undefined)).toBe("default");
  });

  it("the docked spawn panel toggle and side survive a persist then reload", () => {
    settings.setSpawnDock({ enabled: true, side: "left" });

    const loaded = loadFromCache();
    expect(loaded.spawnDock.enabled).toBe(true);
    expect(loaded.spawnDock.side).toBe("left");
  });

  it("the docked spawn panel defaults off on the right and clamps an unknown side", () => {
    expect(mergeDefaults(null).spawnDock).toEqual({
      enabled: false,
      side: "right",
    });
    const merged = mergeDefaults({
      spawnDock: { enabled: "yes", side: "top" },
    } as unknown as Record<string, unknown>);
    expect(merged.spawnDock.enabled).toBe(false);
    expect(merged.spawnDock.side).toBe("right");
  });

  it("the toast position survives a persist then reload", () => {
    settings.setToastPosition("right");

    expect(loadFromCache().toastPosition).toBe("right");
  });

  it("the toast position defaults to center and clamps an unknown value", () => {
    expect(mergeDefaults(null).toastPosition).toBe("center");
    const merged = mergeDefaults({
      toastPosition: "bottom",
    } as unknown as Record<string, unknown>);
    expect(merged.toastPosition).toBe("center");
  });

  it("the docked stats panel survives a persist then reload", () => {
    settings.setStatsDock({ enabled: true, side: "left" });

    const loaded = loadFromCache();
    expect(loaded.statsDock.enabled).toBe(true);
    expect(loaded.statsDock.side).toBe("left");
  });

  it("the docked stats panel defaults off on the right and clamps an unknown side", () => {
    expect(mergeDefaults(null).statsDock).toEqual({
      enabled: false,
      side: "right",
    });
    const merged = mergeDefaults({
      statsDock: { enabled: 1, side: "bottom" },
    } as unknown as Record<string, unknown>);
    expect(merged.statsDock.enabled).toBe(false);
    expect(merged.statsDock.side).toBe("right");
  });

  it("each width maps to a CSS length, the default keeping --content-wide", () => {
    expect(sessionListWidthSize("default")).toBeUndefined();
    expect(sessionListWidthSize("wide")).toBe("80rem");
    expect(sessionListWidthSize("ultra")).toBe("92rem");
    expect(sessionListWidthSize("full")).toBe("100%");
  });

  it("a persisted blob keyed as version restores through the setter path", () => {
    settings.setHarnessMode("oneshot");
    const loaded = loadFromCache();
    expect(loaded.harnessMode).toBe("oneshot");
    expect(CURRENT_VERSION).toBe(1);
  });

  it("missing fields in an older blob fall back to defaults, not undefined", () => {
    const loaded = mergeDefaults({ display: { theme: "light" } } as Record<
      string,
      unknown
    >);
    expect(loaded.display.theme).toBe("light");
    expect(loaded.display.fontScale).toBe(1);
    expect(loaded.display.notifySound).toBe(true);
    expect(loaded.locale).toBeNull();
    expect(loaded.harnessMode).toBe("bg");
  });

  it("clamps an unknown harness mode / locale on load", () => {
    const loaded = mergeDefaults({
      harnessMode: "bogus",
      locale: "zz",
    } as unknown as Record<string, unknown>);
    expect(loaded.harnessMode).toBe("bg");
    expect(loaded.locale).toBeNull();
  });
});

describe("resource monitor (header gauge machines)", () => {
  it("ticked machines survive a persist then reload, in tick order, without duplicates", () => {
    settings.setMonitoredMachine("m-b", true);
    settings.setMonitoredMachine("m-a", true);
    settings.setMonitoredMachine("m-b", true);
    expect(loadFromCache().resourceMonitor.machines).toEqual(["m-b", "m-a"]);
    settings.setMonitoredMachine("m-b", false);
    expect(loadFromCache().resourceMonitor.machines).toEqual(["m-a"]);
    settings.setMonitoredMachine("m-a", false);
  });
  it("an older or corrupt blob yields an empty list", () => {
    expect(mergeDefaults(null).resourceMonitor).toEqual({ machines: [] });
    const merged = mergeDefaults({
      // @ts-expect-error simulating a corrupt stored blob
      resourceMonitor: { machines: ["x", 3, "", "x", null] },
    });
    expect(merged.resourceMonitor.machines).toEqual(["x"]);
  });

  it("macros default to off and empty, and drop malformed entries", () => {
    const d = mergeDefaults(null).macros;
    expect(d).toEqual({ enabled: false, items: [] });
    const merged = mergeDefaults({
      macros: {
        enabled: "yes",
        items: [
          { id: "a", title: " Ménage ", prompt: "go", confirm: true },
          { id: "", title: "sans id", prompt: "go" },
          { id: "b", title: "sans prompt", prompt: "  " },
          "garbage",
        ],
      },
    } as unknown as Record<string, unknown>).macros;
    expect(merged.enabled).toBe(false);
    expect(merged.items.map((m) => m.id)).toEqual(["a"]);
    expect(merged.items[0]).toMatchObject({
      title: "Ménage",
      adapter: "claude-code",
      pool_id: null,
      confirm: true,
    });
    settings.setMacrosEnabled(true);
    settings.setMacros(merged.items);
    expect(loadFromCache().macros.enabled).toBe(true);
    expect(loadFromCache().macros.items).toHaveLength(1);
  });
});

describe("session label preferences", () => {
  it("defaults to full names and preserves explicit legacy account preferences", () => {
    expect(mergeDefaults(null).sessionList).toMatchObject({ machineLabel: "full", accountLabel: "full" });
    for (const [accountNames, expected] of [[true, "full"], [false, "icon"]] as const) {
      expect(mergeDefaults({ sessionList: { accountNames } } as Parameters<typeof mergeDefaults>[0]).sessionList.accountLabel).toBe(expected);
    }
  });
  it("persists display choices and normalizes invalid values", () => {
    settings.setSessionList({ machineLabel: "initial", accountLabel: "3" });
    expect(loadFromCache().sessionList).toMatchObject({ machineLabel: "initial", accountLabel: "3" });
    const loaded = mergeDefaults({ sessionList: { machineLabel: "bad", accountLabel: 42 } } as unknown as Parameters<typeof mergeDefaults>[0]);
    expect(loaded.sessionList).toMatchObject({ machineLabel: "full", accountLabel: "full" });
  });
});
