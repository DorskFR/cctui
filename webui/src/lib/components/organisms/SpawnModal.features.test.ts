// @vitest-environment happy-dom
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Feature } from "@bindings/Feature";
import { setCapabilitiesForTest } from "$lib/features.svelte";
import SpawnModal from "./SpawnModal.svelte";

const calls = vi.hoisted(() => ({
  machinesEnabled: [] as boolean[],
  profilesEnabled: [] as boolean[],
  recentDirsFor: [] as string[],
  gitInfo: [] as string[],
  machineDirs: [] as string[],
  voiceConfig: 0,
  contextItems: 0,
}));

let dispatchers: string[] = [];
let recentDirs: string[] = [];

vi.mock("$lib/queries", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useMachineResources: (enabled: () => boolean) => {
      calls.machinesEnabled.push(enabled());
      return q(
        enabled()
          ? [
              {
                machine_id: "m-uuid-1",
                name: "box",
                display_name: "box",
                hue: null,
                liveness: "online",
                connected: true,
                last_seen_at: "2026-01-01T00:00:00Z",
                resources: null,
                updated_at: null,
              },
            ]
          : undefined,
      );
    },
    useVoiceConfig: () => {
      calls.voiceConfig++;
      return q({ stt: true, tts: true });
    },
    useDispatchers: () => q(dispatchers),
    useSessions: () => q({ sessions: [] }),
    useRecentDirs: (machineId: () => string) => {
      calls.recentDirsFor.push(machineId());
      return q(recentDirs);
    },
    useAccounts: () => q([]),
    useAccountPools: () => q([]),
    useLabels: () => q({ labels: [] }),
    useContextItems: () => {
      calls.contextItems++;
      return q([]);
    },
    useResolvedContext: () => q({ rules: [], memories: [] }),
    useProfiles: (enabled: () => boolean = () => true) => {
      calls.profilesEnabled.push(enabled());
      return q(enabled() ? [] : undefined);
    },
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
    useGitInfo: () => async (machineId: string) => {
      calls.gitInfo.push(machineId);
      return { is_repo: false, is_worktree: false };
    },
    useMachineDirs: () => q([]),
    endpoints: {
      machineDirs: async (id: string) => {
        calls.machineDirs.push(id);
        return { dirs: [] };
      },
    },
  };
});

vi.mock("$lib/settings.svelte", () => ({
  settings: {
    state: { display: { archiveShortcut: true } },
    voice: { autoSend: false },
    lastDirFor: () => null,
    lastEntryFor: () => null,
    recallSpawn: () => null,
    rememberSpawn: () => {},
  },
}));

vi.mock("$lib/ws.svelte", () => ({ ws: { sessions: [] } }));

const ALL: Feature[] = [
  "machine_spawn",
  "files",
  "uploads",
  "drafts",
  "profiles",
  "scheduling",
  "voice",
  "context",
  "federation",
];

function only(off: Feature[]) {
  setCapabilitiesForTest({
    features: ALL.filter((f) => !off.includes(f)),
    switchable: ALL,
  });
}

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  localStorage.clear();
  dispatchers = ["k8s"];
  recentDirs = [];
  calls.machinesEnabled = [];
  calls.profilesEnabled = [];
  calls.recentDirsFor = [];
  calls.gitInfo = [];
  calls.machineDirs = [];
  calls.voiceConfig = 0;
  calls.contextItems = 0;
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  setCapabilitiesForTest(null);
});

async function open() {
  component = mount(SpawnModal, {
    target: document.body,
    props: { onclose: () => {}, onspawned: () => {} },
  });
  await new Promise((r) => setTimeout(r, 100));
}

const buttons = () => [...document.querySelectorAll("button")];
const button = (label: string) =>
  buttons().find((b) => b.textContent?.trim().startsWith(label));
const tabs = () => document.querySelector('[role="radiogroup"]');

describe("SpawnModal with machine_spawn off", () => {
  it("is the dispatch form alone, with no tab strip", async () => {
    only(["machine_spawn"]);
    await open();
    expect(tabs()).toBeNull();
    expect(document.querySelector("#sp-dispatcher")).toBeTruthy();
    expect(document.querySelector("#sp-cwd")).toBeNull();
    expect(button("Dispatch")).toBeTruthy();
    expect(button("Launch")).toBeUndefined();
    expect(button("Draft")).toBeUndefined();
  });

  it("asks for no machine, recent dir or profile", async () => {
    only(["machine_spawn"]);
    await open();
    expect(calls.machinesEnabled.every((e) => e === false)).toBe(true);
    expect(calls.recentDirsFor.every((id) => id === "")).toBe(true);
    expect(calls.profilesEnabled.every((e) => e === false)).toBe(true);
  });

  it("keeps both tabs with every feature on", async () => {
    only([]);
    await open();
    expect(tabs()).toBeTruthy();
    expect(document.querySelector("#sp-cwd")).toBeTruthy();
  });
});

describe("SpawnModal machine tab with a feature off", () => {
  it("drops the file picker without uploads", async () => {
    only(["uploads"]);
    await open();
    expect(document.querySelector('input[type="file"]')).toBeNull();
  });

  it("keeps the file picker with uploads on", async () => {
    only([]);
    await open();
    expect(document.querySelector('input[type="file"]')).toBeTruthy();
  });

  it("drops Draft and the schedule menu without drafts", async () => {
    only(["drafts"]);
    await open();
    expect(button("Draft")).toBeUndefined();
    expect(
      document.querySelector('[aria-label="More spawn options"]'),
    ).toBeNull();
    expect(button("Launch")).toBeTruthy();
  });

  it("drops the schedule menu but keeps Draft without scheduling", async () => {
    only(["scheduling"]);
    await open();
    expect(button("Draft")).toBeTruthy();
    expect(
      document.querySelector('[aria-label="More spawn options"]'),
    ).toBeNull();
  });

  it("asks for no profile without profiles", async () => {
    only(["profiles"]);
    await open();
    expect(calls.profilesEnabled.every((e) => e === false)).toBe(true);
  });

  it("mounts no dictation and asks for no voice config without voice", async () => {
    only(["voice"]);
    await open();
    expect(calls.voiceConfig).toBe(0);
    expect(document.querySelector('[data-journey="spawn-dictation"]')).toBeNull();
  });

  it("mounts no context row without context", async () => {
    only(["context"]);
    await open();
    expect(calls.contextItems).toBe(0);
  });

  it("looks up the cwd branch with files on", async () => {
    only([]);
    recentDirs = ["/srv/project"];
    await open();
    await new Promise((r) => setTimeout(r, 400));
    expect(calls.gitInfo).toContain("m-uuid-1");
  });

  it("looks up no branch without files", async () => {
    only(["files"]);
    recentDirs = ["/srv/project"];
    await open();
    await new Promise((r) => setTimeout(r, 400));
    expect(calls.gitInfo).toEqual([]);
  });
});
