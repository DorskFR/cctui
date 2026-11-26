// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import SpawnModal from "./SpawnModal.svelte";

let machineList: unknown[] = [];

vi.mock("#lib/queries/index.ts", () => {
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
    endpoints: { machineDirs: async () => ({ dirs: [] }) },
  };
});

vi.mock("#lib/settings.svelte.ts", () => ({
  settings: {
    state: { display: { archiveShortcut: true } },
    lastDirFor: () => null,
    lastEntryFor: () => null,
    recallSpawn: () => null,
    rememberSpawn: () => {},
    setSpawnDock: () => {},
  },
}));

vi.mock("#lib/ws.svelte.ts", () => ({ ws: { sessions: [] } }));

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  localStorage.clear();
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
const quiet = () =>
  Array.from(row()?.querySelectorAll<HTMLButtonElement>(".foot-quiet > button") ?? []);

describe("SpawnModal footer row", () => {
  it("puts Clear and Draft as small ghost buttons beside the primary", async () => {
    await open();
    const buttons = quiet();
    expect(buttons.map((b) => b.textContent?.trim())).toEqual(["Clear", "Draft"]);
    for (const b of buttons) {
      expect(b.className).toMatch(/\bbtn-ghost\b/);
      expect(b.className).toMatch(/\bbtn-sm\b/);
    }
    const primary = row()?.lastElementChild as HTMLElement | null;
    expect(primary?.getAttribute("data-journey")).toBe("submit");
    expect(primary?.style.marginInlineStart).toBe("auto");
  });

  it("drops the auto-archive checkbox and its paragraph", async () => {
    await open();
    expect(document.querySelector('input[type="checkbox"][data-journey="auto-archive"]')).toBeNull();
    expect(document.body.textContent).not.toContain(
      "Archives the session once it finishes cleanly",
    );
  });

  it("fills the dock footer", async () => {
    await open({ docked: "right" });
    expect(document.querySelector(".dock-foot .spawn-foot .foot-row")).not.toBeNull();
  });
});
