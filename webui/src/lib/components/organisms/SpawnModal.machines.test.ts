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

async function open() {
  component = mount(SpawnModal, {
    target: document.body,
    props: { onclose: () => {}, onspawned: () => {} },
  });
  await new Promise((r) => setTimeout(r, 100));
}

const cwd = () => document.querySelector<HTMLInputElement>("#sp-cwd");
const text = () => document.body.textContent ?? "";

describe("SpawnModal working directory field", () => {
  it("is a plain path input with a visible placeholder and a real label", async () => {
    await open();
    const input = cwd();
    expect(input).toBeTruthy();
    expect(input?.placeholder).toBe("/home/user/project");
    expect(input?.value).not.toContain("cwd:");
    const label = document.querySelector<HTMLLabelElement>(
      'label[for="sp-cwd"]',
    );
    expect(label?.textContent?.trim()).toBe("Working directory");
  });

  it("completes through a datalist bound to the input", async () => {
    await open();
    expect(cwd()?.getAttribute("list")).toBe("sp-cwd-options");
    expect(document.querySelector("datalist#sp-cwd-options")).toBeTruthy();
  });

  it("picks the machine with a native select carrying an accessible name", async () => {
    await open();
    const select = document.querySelector<HTMLSelectElement>(
      'select[aria-label="Machine"]',
    );
    expect(select).toBeTruthy();
    expect(select?.disabled).toBe(false);
    expect([...(select?.options ?? [])].map((o) => o.value)).toContain(
      "m-uuid-1",
    );
  });
});

describe("SpawnModal with no machines enrolled", () => {
  beforeEach(() => {
    machineList = [];
  });

  it("points at the Overview page and disables both CTAs with that reason", async () => {
    await open();
    const reason = "No machines enrolled — enroll one from the Overview page.";
    expect(text()).toContain(reason);
    expect(
      document.querySelector<HTMLAnchorElement>('a[href="/"]'),
    ).toBeTruthy();

    const buttons = [...document.querySelectorAll("button")];
    const spawn = buttons.find((b) => b.textContent?.includes("Spawn"));
    const draft = buttons.find((b) => b.textContent?.includes("Draft"));
    expect(spawn?.disabled).toBe(true);
    expect(spawn?.title).toBe(reason);
    expect(draft?.disabled).toBe(true);
    expect(draft?.title).toBe(reason);
  });

  it("disables the machine select rather than offering a fake option", async () => {
    await open();
    const select = document.querySelector<HTMLSelectElement>(
      'select[aria-label="Machine"]',
    );
    expect(select?.disabled).toBe(true);
    expect(select?.options.length).toBe(0);
  });
});
