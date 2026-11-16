// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ApiError } from "$lib/api";
import SpawnModal from "./SpawnModal.svelte";

let machineList: unknown[] = [];
const machineDirs = vi.fn(
  async (_machine: string, _path: string): Promise<{ dirs: string[] }> => ({
    dirs: [],
  }),
);

vi.mock("$lib/queries", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useMachineResources: () => q(machineList),
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
    useSessionActions: () => ({}),
    useCodexModels: () => q(null),
    useMergedCodexModels: () => q(null),
    useHarnessModels: () => q(null),
    useHarnesses: () => q(null),
    useMachineAdapters: () => q(null),
    useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
    useMachineDirs: () => q([]),
    endpoints: {
      machineDirs: (machine: string, path: string) => machineDirs(machine, path),
    },
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

const machine = (connected: boolean) => ({
  machine_id: "m-uuid-1",
  name: "box",
  display_name: "Box",
  hue: null,
  liveness: connected ? "online" : "offline",
  connected,
  last_seen_at: "2026-01-01T00:00:00Z",
  resources: null,
  updated_at: null,
});

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  localStorage.clear();
  machineDirs.mockReset();
  machineDirs.mockImplementation(async () => ({ dirs: [] }));
  machineList = [machine(true)];
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

const tick = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function open(working_dir: string) {
  component = mount(SpawnModal, {
    target: document.body,
    props: {
      onclose: () => {},
      onspawned: () => {},
      prefill: { machine_id: "m-uuid-1", working_dir, prompt: "go" },
      autosaveDelay: 10_000,
    },
  });
  await tick(450);
}

const text = () => document.body.textContent ?? "";
const spawnButton = () => {
  const b = [...document.querySelectorAll<HTMLButtonElement>("button")].find(
    (x) => x.textContent?.trim().startsWith("Launch"),
  );
  if (!b) throw new Error("no Launch button");
  return b;
};
const NOT_FOUND = "Directory not found on Box";
const OFFLINE = "Can't check the directory: Box is offline";

describe("SpawnModal working directory existence check", () => {
  it("flags a directory the machine does not have and disables Launch", async () => {
    machineDirs.mockImplementation(async () => {
      throw new ApiError(404, "cannot resolve /nope: No such file or directory (os error 2)");
    });
    await open("/nope");
    expect(machineDirs).toHaveBeenCalledWith("m-uuid-1", "/nope");
    expect(text()).toContain(NOT_FOUND);
    expect(spawnButton().disabled).toBe(true);
  });

  it("enables Launch for a directory that exists", async () => {
    await open("/w");
    expect(machineDirs).toHaveBeenCalledWith("m-uuid-1", "/w");
    expect(text()).not.toContain(NOT_FOUND);
    expect(text()).not.toContain(OFFLINE);
    expect(spawnButton().disabled).toBe(false);
  });

  it("only warns, without blocking, when the machine is offline", async () => {
    machineList = [machine(false)];
    await open("/w");
    expect(machineDirs).not.toHaveBeenCalledWith("m-uuid-1", "/w");
    expect(text()).toContain(OFFLINE);
    expect(text()).not.toContain(NOT_FOUND);
    expect(spawnButton().disabled).toBe(false);
  });

  it("does not block on an answer that says nothing about existence", async () => {
    machineDirs.mockImplementation(async () => {
      throw new ApiError(400, "/srv is outside the allowed roots");
    });
    await open("/srv");
    expect(text()).not.toContain(NOT_FOUND);
    expect(spawnButton().disabled).toBe(false);
  });
});
