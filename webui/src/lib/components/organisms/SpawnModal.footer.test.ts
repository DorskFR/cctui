// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SpawnModal from "./SpawnModal.svelte";

let machineList: unknown[] = [];

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
    useSessionActions: () => ({}),
    useCodexModels: () => q(null),
    useMergedCodexModels: () => q(null),
    useHarnessModels: () => q(null),
    useHarnesses: () => q(null),
    useMachineAdapters: () => q(null),
    useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
    useMachineDirs: () => q([]),
    endpoints: { machineDirs: async () => ({ dirs: [] }) },
  };
});

vi.mock("$lib/settings.svelte", () => ({
  settings: {
    state: { display: { archiveShortcut: true } },
    lastDirFor: () => null,
    lastEntryFor: () => null,
    recallSpawn: () => null,
    rememberSpawn: () => {},
    setSpawnDock: () => {},
  },
}));

vi.mock("$lib/ws.svelte", () => ({ ws: { sessions: [] } }));

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  localStorage.clear();
  machineList = [
    {
      id: "m-uuid-1",
      name: "box",
      display_name: "box",
      kind: "persistent",
      hue: null,
    },
  ];
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

async function open(props: Record<string, unknown> = {}) {
  component = mount(SpawnModal, {
    target: document.body,
    props: { onclose: () => {}, onspawned: () => {}, ...props },
  });
  await new Promise((r) => setTimeout(r, 100));
}

const row = () => document.querySelector<HTMLElement>(".foot-row");
const growers = () =>
  Array.from(row()?.children ?? []).map((n) =>
    (n as HTMLElement).style.flex.replace(/\s+/g, " ").trim(),
  );

describe("SpawnModal footer row", () => {
  it("holds every action in one row, each growing with its content", async () => {
    await open();
    expect(row()).not.toBeNull();
    const flexes = growers();
    expect(flexes.length).toBeGreaterThanOrEqual(2);
    expect(flexes.every((f) => f.startsWith("1 1"))).toBe(true);
    expect(flexes.some((f) => f === "1 1 auto")).toBe(true);
    expect(flexes.at(-1)).toBe("1 1 11rem");
  });

  it("keeps the caret attached to a growing primary segment", async () => {
    await open();
    const split = row()?.querySelector<HTMLElement>('[data-tsu="SplitButton"]');
    expect(split).not.toBeNull();
    expect(split?.style.gridTemplateColumns).toBe("1fr auto");
  });

  it("fills the row when the dock renders it", async () => {
    await open({ docked: "right" });
    expect(document.querySelector(".dock-foot .foot-row")).not.toBeNull();
  });
});
