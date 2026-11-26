// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SpawnModal from "./SpawnModal.svelte";
import { spawnSlotKey } from "#lib/drafts.ts";
import { attachmentStore } from "#lib/attachmentStore.ts";
import { schedulePresets } from "./conversation/scheduleTimes";

let machineList: Array<Record<string, unknown>> = [
  {
    machine_id: "m-uuid-1",
    name: "box",
    display_name: "box",
    hue: null,
    liveness: "online",
    last_seen_at: "2026-01-01T00:00:00Z",
    resources: null,
    updated_at: null,
  },
];
const spawn = vi.fn();
const updateDraft = vi.fn();
const scheduleDraftLaunch = vi.fn();

vi.mock("#lib/queries/index.ts", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useMachineResources: () => q(machineList),
    useMe: () => q(undefined),
    useVoiceConfig: () => q(undefined),
    useDispatchers: () => q([]),
    useSessions: () => q({ sessions: [] }),
    useRecentDirs: () => q([]),
    useAccounts: () => q([]),
    useAccountPools: () => q([]),
    useLabels: () => q({ labels: [] }),
    useContextItems: () => q([]),
    useResolvedContext: () => q({ rules: [], memories: [] }),
    useProfiles: () => q([]),
    useProfileActions: () => ({
      create: async () => ({ id: "p-1", name: "Default" }),
      update: async () => ({}),
      remove: async () => {},
    }),
    useAllAccountsUsage: () => q([]),
    useSessionActions: () => ({
      spawn,
      updateDraft,
      scheduleDraftLaunch,
      discardDraft: async () => {},
    }),
    useCodexModels: () => q(null),
    useMergedCodexModels: () => q(null),
    useHarnessModels: () => q(null),
    useHarnesses: () => q(null),
    useMachineAdapters: () => q(null),
    useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
    useMachineDirs: () => q([]),
    endpoints: { machineDirs: async () => [] },
  };
});

vi.mock("#lib/settings.svelte.ts", () => ({
  settings: {
    state: { display: { archiveShortcut: true } },
    lastDirFor: () => null,
    lastEntryFor: () => null,
    recallSpawn: () => null,
    rememberSpawn: () => {},
  },
}));

vi.mock("#lib/ws.svelte.ts", () => ({ ws: { sessions: [] } }));

const SLOT = spawnSlotKey("m-uuid-1", "/w");
let component: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
  machineList = [
    {
      machine_id: "m-uuid-1",
      name: "box",
      display_name: "box",
      hue: null,
      liveness: "online",
      last_seen_at: "2026-01-01T00:00:00Z",
      resources: null,
      updated_at: null,
    },
  ];
  localStorage.clear();
  await attachmentStore.clearAll();
  spawn.mockReset().mockResolvedValue({
    command_id: "draft-1",
    status: "draft",
    account: null,
  });
  updateDraft.mockReset().mockResolvedValue({
    command_id: "draft-1",
    status: "draft",
    account: null,
  });
  scheduleDraftLaunch.mockReset().mockResolvedValue(undefined);
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

const tick = (ms = 50) => new Promise((r) => setTimeout(r, ms));

// `seedSlot: false` leaves the form without a machine/cwd, the state a first
// visit is in — `spawnValid` reads the form, not the machine list, so a seeded
// slot keeps the CTA enabled even with nothing enrolled.
async function open({ seedSlot = true }: { seedSlot?: boolean } = {}) {
  if (seedSlot) {
    localStorage.setItem(
      SLOT,
      JSON.stringify({ machine_id: "m-uuid-1", working_dir: "/w" }),
    );
    localStorage.setItem("cctui_spawn_slot", SLOT);
  }
  component = mount(SpawnModal, {
    target: document.body,
    props: {
      onclose: () => {},
      onspawned: () => {},
      prefill: null,
      // Long enough that the autosave timer never races the schedule click.
      autosaveDelay: 100_000,
    },
  });
  await tick(100);
}

function field(id: string): HTMLInputElement | HTMLTextAreaElement {
  const el = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(
    `#${id}`,
  );
  if (!el) throw new Error(`#${id} not found`);
  return el;
}
async function typeInto(
  el: HTMLInputElement | HTMLTextAreaElement,
  value: string,
) {
  el.value = value;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
}

const caret = () =>
  document.querySelector<HTMLButtonElement>(
    'button[aria-label="More spawn options"]',
  );

// happy-dom implements no Popover API, so the caret's click never opens the
// panel: drive its `toggle` by hand, the way PromptHistoryMenu.test.ts does.
async function openMenu() {
  const trigger = caret();
  if (!trigger) throw new Error("caret did not render");
  // An id is not necessarily a valid CSS selector, so resolve it by id.
  const target = trigger.getAttribute("popovertarget");
  const panel = (target ? document.getElementById(target) : null) ??
    document.querySelector("[popover]");
  if (!panel) throw new Error("panel did not render");
  (panel as HTMLElement & { hidePopover?: () => void }).hidePopover = () => {};
  panel.dispatchEvent(
    Object.assign(new Event("toggle"), { newState: "open" }),
  );
  await tick();
  flushSync();
  const menu = document.querySelector('[role="menu"]');
  if (!menu) throw new Error("menu did not render");
  return menu;
}

const items = () => [
  ...document.querySelectorAll<HTMLElement>(
    '[role="menu"] [role="menuitem"]',
  ),
];
const menuItem = (text: string) =>
  items().find((b) => (b.textContent ?? "").includes(text));

describe("SpawnModal schedule split-button", () => {
  it("offers the composer's presets on the Spawn caret", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    await openMenu();
    // One entry per preset, the custom time and Repeat…; no pending-message list here.
    expect(items()).toHaveLength(schedulePresets(new Date()).length + 2);
    expect(menuItem("Repeat")).toBeDefined();
  });

  it("saves the draft and queues it at the picked preset", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    await openMenu();
    items()[0].click();
    await tick(120);

    expect(spawn).toHaveBeenCalledTimes(1);
    expect(spawn.mock.calls[0][0]).toMatchObject({ save_draft: true });
    expect(scheduleDraftLaunch).toHaveBeenCalledTimes(1);
    const [draftId, launchAt] = scheduleDraftLaunch.mock.calls[0];
    expect(draftId).toBe("draft-1");
    const at = new Date(launchAt as string);
    expect(at.getTime()).toBeGreaterThan(Date.now());
    expect(at.toISOString()).toBe(launchAt);
  });

  it("opens the composer's custom-time picker, not a copy of it", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    await openMenu();
    menuItem("Custom")?.click();
    await tick();
    expect(
      document.querySelector<HTMLInputElement>('input[type="datetime-local"]'),
    ).toBeTruthy();
    expect(scheduleDraftLaunch).not.toHaveBeenCalled();
  });

  it("ends the menu with an auto-archive toggle that launches with it", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    await openMenu();
    const toggle = document.querySelector<HTMLElement>(
      '[role="menuitemcheckbox"][data-journey="auto-archive"]',
    );
    expect(toggle).not.toBeNull();
    expect(toggle?.parentElement?.lastElementChild).toBe(toggle);
    expect(toggle?.parentElement?.querySelector('[role="menuitem"]')).not.toBeNull();
    expect(toggle?.textContent).toContain("Auto-archive when done");
    expect(toggle?.title).toContain("Archives the session once it finishes cleanly");
    expect(toggle?.getAttribute("aria-checked")).toBe("false");
    expect(document.querySelector('[data-journey="auto-archive-on"]')).toBeNull();

    toggle?.click();
    await tick();
    flushSync();
    const on = document.querySelector<HTMLElement>('[role="menuitemcheckbox"][data-journey="auto-archive"]');
    expect(on?.getAttribute("aria-checked")).toBe("true");
    expect(document.querySelector('[data-journey="auto-archive-on"]')).not.toBeNull();

    document
      .querySelector<HTMLButtonElement>('[data-journey="submit"] .split-main button')
      ?.click();
    await tick(120);
    expect(spawn).toHaveBeenCalledTimes(1);
    expect(spawn.mock.calls[0][0]).toMatchObject({ auto_archive: true });
  });

  it("toggles auto-archive back off from the menu", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    await openMenu();
    const toggle = () =>
      document.querySelector<HTMLElement>('[role="menuitemcheckbox"][data-journey="auto-archive"]');
    toggle()?.click();
    await tick();
    toggle()?.click();
    await tick();
    flushSync();
    expect(toggle()?.getAttribute("aria-checked")).toBe("false");
    expect(document.querySelector('[data-journey="auto-archive-on"]')).toBeNull();
  });

  it("falls back to a plain titled Spawn button with no machines", async () => {
    machineList = [];
    await open({ seedSlot: false });
    expect(caret()).toBeNull();
    const reason = "No machines enrolled — enroll one with a user token.";
    const spawn = [...document.querySelectorAll("button")].find((b) =>
      b.textContent?.includes("Launch"),
    );
    expect(spawn?.disabled).toBe(true);
    expect(spawn?.title).toBe(reason);
  });
});
