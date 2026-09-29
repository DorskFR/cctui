// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SpawnModal from "./SpawnModal.svelte";
import { spawnSlotKey } from "$lib/drafts";
import { attachmentStore } from "$lib/attachmentStore";
import { schedulePresets } from "./conversation/scheduleTimes";

const machineList = [
  {
    id: "m-uuid-1",
    name: "box",
    display_name: "box",
    kind: "persistent",
    hue: null,
  },
];
const spawn = vi.fn();
const updateDraft = vi.fn();
const scheduleDraftLaunch = vi.fn();

vi.mock("$lib/queries", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useAllMachines: () => q(machineList),
    useDispatchers: () => q([]),
    useSessions: () => q({ sessions: [] }),
    useRecentDirs: () => q([]),
    useAccounts: () => q([]),
    useAccountPools: () => q([]),
    useLabels: () => q({ labels: [] }),
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
    useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
    useMachineDirs: () => q([]),
    endpoints: { machineDirs: async () => [] },
  };
});

vi.mock("$lib/settings.svelte", () => ({
  settings: {
    state: { display: { archiveShortcut: true } },
    lastDirFor: () => null,
    lastEntryFor: () => null,
    recallSpawn: () => null,
    rememberSpawn: () => {},
  },
}));

vi.mock("$lib/ws.svelte", () => ({ ws: { sessions: [] } }));

const SLOT = spawnSlotKey("m-uuid-1", "/w");
let component: ReturnType<typeof mount> | undefined;

beforeEach(async () => {
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

async function open() {
  localStorage.setItem(
    SLOT,
    JSON.stringify({ machine_id: "m-uuid-1", working_dir: "/w" }),
  );
  localStorage.setItem("cctui_spawn_slot", SLOT);
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
const menuItem = (text: string) =>
  [...document.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitem"]')].find((b) =>
    (b.textContent ?? "").includes(text),
  );

describe("SpawnModal schedule split-button", () => {
  it("offers the composer's presets on the Spawn caret", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    const trigger = caret();
    expect(trigger).toBeTruthy();
    trigger?.click();
    await tick();
    const labels = [
      ...document.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitem"]'),
    ].map((b) => b.textContent ?? "");
    // One entry per preset plus the custom time; no pending-message list here.
    expect(labels).toHaveLength(schedulePresets(new Date()).length + 1);
  });

  it("saves the draft and queues it at the picked preset", async () => {
    await open();
    await typeInto(field("sp-prompt"), "do the thing");
    caret()?.click();
    await tick();
    const first = [
      ...document.querySelectorAll<HTMLElement>('[role="menu"] [role="menuitem"]'),
    ][0];
    first.click();
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
    caret()?.click();
    await tick();
    menuItem("Custom")?.click();
    await tick();
    expect(
      document.querySelector<HTMLInputElement>('input[type="datetime-local"]'),
    ).toBeTruthy();
    expect(scheduleDraftLaunch).not.toHaveBeenCalled();
  });

  it("keeps a plain Spawn button on the dispatch target", async () => {
    await open();
    const radio = document.querySelector<HTMLInputElement>(
      'input[type="radio"][value="dispatch"]',
    );
    if (!radio) return;
    radio.click();
    await tick();
    expect(caret()).toBeNull();
  });
});
