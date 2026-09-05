import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionProfile } from "@bindings/SessionProfile";
import SpawnModal from "./SpawnModal.svelte";

const machineList = [
  {
    id: "m-uuid-1",
    name: "box",
    display_name: "box",
    kind: "persistent",
    hue: null,
  },
];
const accounts = [
  {
    id: "a1",
    name: "personal",
    emoji: "🐼",
    providers: [
      { id: "pr1", provider: "anthropic", models: [], model_aliases: null },
    ],
  },
];
const orchestrator: SessionProfile = {
  id: "p1",
  user_id: "u1",
  name: "Orchestrator",
  harness: "claude-code",
  account_id: "a1",
  pool_id: null,
  no_account: false,
  model_alias: "fable",
  effort: "medium",
  permission_mode: "yolo",
  created_at: "",
  updated_at: "",
};
const codexQuick: SessionProfile = {
  ...orchestrator,
  id: "p2",
  name: "Codex quick",
  harness: "codex",
  account_id: null,
  model_alias: null,
  effort: "high",
  permission_mode: "auto",
};
let profileList: SessionProfile[] = [];
let lastEntry: { profile_id?: string } | null = null;
const spawn = vi.fn();
const create = vi.fn();
const update = vi.fn();
const remember = vi.fn();

vi.mock("$lib/queries", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useAllMachines: () => q(machineList),
    useDispatchers: () => q([]),
    useSessions: () => q({ sessions: [] }),
    useRecentDirs: () => q([]),
    useAccounts: () => q(accounts),
    useAccountPools: () => q([{ id: "pool1", name: "shared", members: [] }]),
    useLabels: () => q({ labels: [] }),
    useProfiles: () => q(profileList),
    useProfileActions: () => ({ create, update, remove: async () => {} }),
    useAllAccountsUsage: () => q([]),
    useSessionActions: () => ({
      spawn,
      updateDraft: async () => ({}),
      discardDraft: async () => {},
    }),
    useCodexModels: () => q(null),
    useMergedCodexModels: () => q(null),
    useGitInfo: () => async () => ({ is_repo: false, is_worktree: false }),
    useMachineDirs: () => q([]),
    endpoints: {
      machineDirs: async () => [],
      sessions: async () => ({ sessions: [] }),
    },
  };
});

vi.mock("$lib/settings.svelte", () => ({
  settings: {
    state: { display: { archiveShortcut: true } },
    lastDirFor: () => null,
    lastEntryFor: () => lastEntry,
    recallSpawn: () => null,
    rememberSpawn: (...args: unknown[]) => remember(...args),
  },
}));

vi.mock("$lib/ws.svelte", () => ({
  ws: { sessions: [], awaitCommand: async () => ({ ok: true }) },
}));

let component: ReturnType<typeof mount> | undefined;

beforeEach(() => {
  localStorage.clear();
  profileList = [orchestrator, codexQuick];
  lastEntry = null;
  spawn
    .mockReset()
    .mockResolvedValue({ command_id: "c-1", status: "ok", account: null });
  create
    .mockReset()
    .mockResolvedValue({ ...orchestrator, id: "p-new", name: "Default" });
  update.mockReset().mockResolvedValue(orchestrator);
  remember.mockReset();
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

const tick = (ms = 60) => new Promise((r) => setTimeout(r, ms));

async function open() {
  component = mount(SpawnModal, {
    target: document.body,
    props: {
      onclose: () => {},
      onspawned: () => {},
      prefill: { machine_id: "m-uuid-1", working_dir: "/w", prompt: "go" },
      autosaveDelay: 10_000,
    },
  });
  await tick(100);
}

const buttons = () => [
  ...document.querySelectorAll<HTMLButtonElement>("button"),
];
const button = (text: string | RegExp) => {
  const b = buttons().find((x) => {
    const label = x.textContent?.trim() || x.getAttribute("aria-label") || "";
    return typeof text === "string" ? label === text : text.test(label);
  });
  if (!b) throw new Error(`button ${text} not found`);
  return b;
};
const spawnButton = () => button(/^Spawn/);
// The button's text minus the Kbd chord hint that follows the label.
const mode = (v: string) => {
  const el = document.querySelector<HTMLButtonElement>(
    `button[data-mode="${v}"]`,
  );
  if (!el) throw new Error(`permission mode ${v} not found`);
  return el;
};
const radio = (id: string) => {
  const el = document.querySelector<HTMLInputElement>(`#sp-profile-${id}`);
  if (!el) throw new Error(`profile radio ${id} not found`);
  return el;
};

async function submit() {
  spawnButton().click();
  await tick(80);
  expect(spawn).toHaveBeenCalledTimes(1);
  return spawn.mock.calls[0][0];
}

describe("SpawnModal profiles", () => {
  it("lists the profiles, selects the first and names it on the Spawn button", async () => {
    await open();
    expect(radio("p1").checked).toBe(true);
    expect(radio("p2").checked).toBe(false);
    expect(document.body.textContent).toContain(
      "Claude Code · 🐼 personal · Fable · medium · Yolo",
    );
  });

  it("spawns with the selected profile merged under the prompt and where", async () => {
    await open();
    const body = await submit();
    expect(body).toMatchObject({
      machine_id: "m-uuid-1",
      working_dir: "/w",
      prompt: "go",
      adapter_id: "claude-code",
      account: "personal",
      provider: "anthropic",
      model: "fable",
      effort: "medium",
      permission_mode: "yolo",
    });
    expect(remember.mock.calls[0][1]).toMatchObject({
      profile_id: "p1",
      account: "personal",
    });
    expect(
      JSON.parse(localStorage.getItem("cctui_profile_uses") ?? "{}").p1,
    ).toHaveLength(1);
  });

  it("opens on the machine's last-used profile and switches on radio pick", async () => {
    lastEntry = { profile_id: "p2" };
    await open();
    expect(radio("p2").checked).toBe(true);
    radio("p1").click();
    await tick();
    radio("p2").click();
    await tick();
    const body = await submit();
    expect(body).toMatchObject({
      adapter_id: "codex",
      account: null,
      auto_account: true,
      effort: "high",
      permission_mode: "auto",
    });
  });

  it("with no profile it renders the bare kit and settles (no effect loop)", async () => {
    profileList = [];
    await open();
    await tick(120);
    // An unguarded mirror of `oneOff` re-entered until Svelte threw
    // effect_update_depth_exceeded and the tab died.
    expect(mode("ask")).toBeTruthy();
    expect(
      document.querySelector('select[id^="sp-kit-account"]'),
    ).not.toBeNull();
    // The kit is editable and its writes land on the one-off spec.
    mode("yolo").click();
    await tick();
    expect(mode("yolo").getAttribute("aria-checked")).toBe("true");
  });

  it('"Use once" adjusts this run only and marks the Spawn button', async () => {
    await open();
    button("Adjust profile").click();
    await tick();
    mode("ask").click();
    await tick();
    expect(document.body.textContent).toContain("1 changes");
    button("Use once").click();
    await tick();
    expect(update).not.toHaveBeenCalled();
    const body = await submit();
    expect(body).toMatchObject({ permission_mode: "ask", model: "fable" });
  });

  it('"Save to profile" persists the adjusted kit', async () => {
    await open();
    button("Adjust profile").click();
    await tick();
    mode("auto").click();
    await tick();
    button("Save to profile").click();
    await tick();
    expect(update).toHaveBeenCalledTimes(1);
    expect(update.mock.calls[0][0]).toBe("p1");
    expect(update.mock.calls[0][1]).toMatchObject({
      name: "Orchestrator",
      spec: {
        harness: "claude-code",
        account_id: "a1",
        permission_mode: "auto",
      },
    });
  });

  it("spawns inside the profile's pool, and unbound for a no-account profile", async () => {
    profileList = [
      { ...orchestrator, account_id: null, pool_id: "pool1" },
      { ...codexQuick, no_account: true },
    ];
    await open();
    expect(document.body.textContent).toContain("Claude Code · shared · Fable");
    let body = await submit();
    expect(body).toMatchObject({
      pool: "shared",
      account: null,
      auto_account: false,
      no_account: false,
    });

    await unmount(component as NonNullable<typeof component>);
    component = undefined;
    document.body.replaceChildren();
    spawn.mockClear();
    lastEntry = { profile_id: "p2" };
    await open();
    body = await submit();
    expect(body).toMatchObject({
      pool: null,
      account: null,
      no_account: true,
      auto_account: false,
    });
  });

  it('offers pools and "no account" in the adjust panel and persists the pick', async () => {
    await open();
    button("Adjust profile").click();
    await tick();
    const select = document.querySelector<HTMLSelectElement>(
      'select[id^="sp-kit-account"]',
    );
    if (!select) throw new Error("account select not found");
    const values = [...select.options].map((o) => o.value);
    expect(values).toEqual(["", "\x00no-account", "\x00pool:pool1", "a1"]);
    const pick = select.options[2];
    pick.selected = true;
    // jsdom cannot match `option:checked`, which Svelte's select binding reads.
    const nativeQuery = select.querySelector.bind(select);
    select.querySelector = ((q: string) =>
      q === ":checked" ? pick : nativeQuery(q)) as typeof select.querySelector;
    select.dispatchEvent(new Event("change", { bubbles: true }));
    await tick();
    button("Save to profile").click();
    await tick();
    expect(update.mock.calls[0][1]).toMatchObject({
      spec: { account_id: null, pool_id: "pool1", no_account: false },
    });
  });

  it('seeds a "Default" profile from the form when the user has none', async () => {
    profileList = [];
    await open();
    expect(create).toHaveBeenCalledTimes(1);
    expect(create.mock.calls[0][0]).toMatchObject({
      name: "Default",
      harness: "claude-code",
    });
  });
});
