// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SessionProfile } from "@bindings/SessionProfile";
import SpawnModal from "./SpawnModal.svelte";

const machineList = [
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
  service_tier: null,
  context_items: [],
  sort_order: 0,
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
const reorder = vi.fn();

vi.mock("$lib/queries", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useMachineResources: () => q(machineList),
    useVoiceConfig: () => q(undefined),
    useDispatchers: () => q([]),
    useSessions: () => q({ sessions: [] }),
    useRecentDirs: () => q([]),
    useAccounts: () => q(accounts),
    useAccountPools: () => q([{ id: "pool1", name: "shared", members: [] }]),
    useLabels: () => q({ labels: [] }),
    useContextItems: () => q([]),
    useResolvedContext: () => q({ rules: [], memories: [] }),
    useProfiles: () => q(profileList),
    useProfileActions: () => ({ create, update, remove: async () => {}, reorder }),
    useAllAccountsUsage: () => q([]),
    useSessionActions: () => ({
      spawn,
      updateDraft: async () => ({}),
      discardDraft: async () => {},
    }),
    useCodexModels: () => q(null),
    useMergedCodexModels: () => q(null),
    useHarnessModels: () => q(null),
    useHarnesses: () => q(null),
    useMachineAdapters: () => q(null),
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
  ws: { sessions: [], awaitSpawn: async () => ({ ok: true }) },
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
  reorder.mockReset().mockResolvedValue([codexQuick, orchestrator]);
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
  stubPopover();
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
const spawnButton = () => button(/^Launch/);
// The button's text minus the Kbd chord hint that follows the label.
const mode = (v: string) => {
  const el = document.querySelector<HTMLButtonElement>(
    `button[data-mode="${v}"]`,
  );
  if (!el) throw new Error(`permission mode ${v} not found`);
  return el;
};
// happy-dom implements no Popover API: the panel neither answers
// show/hidePopover nor fires its own toggle, so both are wired to dispatch the
// toggle a browser would.
const trigger = () => {
  const el = document.querySelector<HTMLButtonElement>(
    '[data-journey="profiles"] .rs-trigger',
  );
  if (!el) throw new Error("profile picker trigger not found");
  return el;
};
const panel = () => {
  const id = trigger().getAttribute("popovertarget");
  const el = id ? document.getElementById(id) : null;
  if (!el) throw new Error("profile picker panel not found");
  return el as HTMLElement & {
    showPopover: () => void;
    hidePopover: () => void;
  };
};
function stubPopover() {
  if (profileList.length === 0) return;
  const el = panel();
  const fire = (newState: "open" | "closed") =>
    el.dispatchEvent(Object.assign(new Event("toggle"), { newState }));
  el.showPopover = () => fire("open");
  el.hidePopover = () => fire("closed");
}
const expanded = () => trigger().getAttribute("aria-expanded") === "true";
async function clickPicker() {
  trigger().click();
  panel().showPopover();
  await tick();
  flushSync();
}

const cards = () => [
  ...document.querySelectorAll<HTMLElement>('[data-journey="profile"]'),
];
const selectedCard = () =>
  trigger().querySelector<HTMLElement>('[data-journey="profile"]')
    ?.dataset.profileId ?? null;
const options = () => [
  ...document.querySelectorAll<HTMLElement>('[role="listbox"] [role="option"]'),
];
const optionNames = () =>
  options().map((o) =>
    o.querySelector(".name .truncate")?.textContent?.trim(),
  );
const option = (id: string) => {
  const el = options().find((o) =>
    o.querySelector(`[data-profile-id="${id}"]`),
  );
  if (!el) throw new Error(`profile option ${id} not found`);
  return el;
};
async function pickProfile(id: string) {
  await clickPicker();
  option(id).click();
  await tick();
  flushSync();
}
const key = (
  el: HTMLElement,
  k: string,
  mods: Partial<KeyboardEventInit> = {},
) => {
  const ev = new KeyboardEvent("keydown", {
    key: k,
    bubbles: true,
    cancelable: true,
    ...mods,
  });
  el.dispatchEvent(ev);
  return ev;
};
const live = () =>
  [...document.querySelectorAll('[aria-live="polite"]')]
    .map((n) => n.textContent?.trim() ?? "")
    .find((t) => t.includes("moved to position")) ?? "";

async function submit() {
  spawnButton().click();
  await tick(80);
  expect(spawn).toHaveBeenCalledTimes(1);
  return spawn.mock.calls[0][0];
}

describe("SpawnModal profiles", () => {
  it("closed, it shows only the selected profile as one card", async () => {
    await open();
    expect(selectedCard()).toBe("p1");
    expect(cards()).toHaveLength(1);
    expect(options()).toHaveLength(0);
    expect(expanded()).toBe(false);
    expect(trigger().getAttribute("aria-haspopup")).toBe("listbox");
    expect(trigger().textContent).toContain(
      "Claude Code · 🐼 personal · Fable · medium · Yolo",
    );
  });

  it("click opens the popover with every profile card, a usage hint, a gear per row and the New profile footer", async () => {
    localStorage.setItem(
      "cctui_profile_uses",
      JSON.stringify({ p2: [Date.now() - 1000] }),
    );
    await open();
    await clickPicker();
    expect(expanded()).toBe(true);
    expect(optionNames()).toEqual(["Orchestrator", "Codex quick"]);
    expect(option("p1").getAttribute("aria-selected")).toBe("true");
    expect(option("p2").getAttribute("aria-selected")).toBe("false");
    expect(option("p2").textContent).toMatch(/this week|today|ago/);
    const rows = [...panel().querySelectorAll(".rs-row")];
    for (const row of rows) {
      expect(row.querySelector(".rs-grip")).not.toBeNull();
      expect(
        row.querySelector('.rs-trailing button[aria-label="Adjust profile"]'),
      ).not.toBeNull();
    }
    const footer = panel().querySelector(".rs-footer");
    expect(footer?.textContent?.trim()).toBe("New profile");
    expect(panel().querySelector(".rs-list")?.contains(footer ?? null)).toBe(
      false,
    );
  });

  it("selecting a card closes the popover back to one card and launches with it", async () => {
    await open();
    await pickProfile("p2");
    expect(expanded()).toBe(false);
    expect(selectedCard()).toBe("p2");
    expect(
      cards().filter((c) => trigger().contains(c)).map((c) => c.dataset.profileId),
    ).toEqual(["p2"]);
    const body = await submit();
    expect(body).toMatchObject({
      adapter_id: "codex",
      account: null,
      auto_account: true,
      effort: "high",
      permission_mode: "auto",
    });
  });

  it("arrow keys on the closed picker switch profiles without opening it", async () => {
    await open();
    trigger().focus();
    const down = key(trigger(), "ArrowDown");
    await tick();
    expect(down.defaultPrevented).toBe(true);
    expect(selectedCard()).toBe("p2");
    expect(expanded()).toBe(false);
    expect(options()).toHaveLength(0);

    key(trigger(), "ArrowUp");
    await tick();
    expect(selectedCard()).toBe("p1");

    key(trigger(), "End");
    await tick();
    expect(selectedCard()).toBe("p2");

    key(trigger(), "Home");
    await tick();
    expect(selectedCard()).toBe("p1");
    expect(expanded()).toBe(false);

    key(trigger(), "ArrowDown");
    await tick();
    const body = await submit();
    expect(body).toMatchObject({ adapter_id: "codex" });
  });

  it("Cmd+Enter and Ctrl+Enter on the picker still launch, without opening it", async () => {
    await open();
    trigger().focus();
    const cmd = key(trigger(), "Enter", { metaKey: true });
    await tick(80);
    expect(cmd.defaultPrevented).toBe(true);
    expect(spawn).toHaveBeenCalledTimes(1);
    expect(spawn.mock.calls[0][0]).toMatchObject({ adapter_id: "claude-code" });
    expect(expanded()).toBe(false);

    await unmount(component as NonNullable<typeof component>);
    component = undefined;
    document.body.replaceChildren();
    spawn.mockClear();
    await open();
    trigger().focus();
    key(trigger(), "Enter", { ctrlKey: true });
    await tick(80);
    expect(spawn).toHaveBeenCalledTimes(1);
  });

  it("Escape closes the open picker and keeps the selection", async () => {
    await open();
    await clickPicker();
    trigger().focus();
    key(trigger(), "ArrowDown");
    key(trigger(), "Escape");
    await tick();
    flushSync();
    expect(expanded()).toBe(false);
    expect(selectedCard()).toBe("p1");
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

  it("opens on the machine's last-used profile", async () => {
    lastEntry = { profile_id: "p2" };
    await open();
    expect(selectedCard()).toBe("p2");
    await pickProfile("p1");
    expect(selectedCard()).toBe("p1");
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

  it('"Use once" adjusts this run only and marks the Launch button', async () => {
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

  it("the gear on a popover row closes it and adjusts that profile without selecting it", async () => {
    await open();
    await clickPicker();
    const gear = option("p2")
      .closest(".rs-row")
      ?.querySelector<HTMLButtonElement>('.rs-trailing button[aria-label="Adjust profile"]');
    if (!gear) throw new Error("gear not found");
    gear.click();
    await tick();
    flushSync();
    expect(expanded()).toBe(false);
    expect(selectedCard()).toBe("p1");
    const adjust = document.querySelector<HTMLElement>(
      '[data-journey="profile-adjust"]',
    );
    expect(adjust?.dataset.profileId).toBe("p2");
    expect(
      adjust?.querySelector<HTMLInputElement>('input[id="sp-profile-name-p2"]')
        ?.value,
    ).toBe("Codex quick");
  });

  it("the New profile footer closes the popover and creates a profile", async () => {
    await open();
    await clickPicker();
    const footer = panel().querySelector<HTMLButtonElement>(".rs-footer button");
    if (!footer) throw new Error("footer button not found");
    footer.click();
    await tick();
    flushSync();
    expect(expanded()).toBe(false);
    expect(create).toHaveBeenCalledTimes(1);
    expect(create.mock.calls[0][0]).toMatchObject({ name: "Profile" });
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
    expect(values).toEqual(["\x00pool:pool1", "a1", "", "\x00no-account"]);
    const pick = select.options[0];
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

  it("reorders with Alt+Arrow in the open popover and persists the new order", async () => {
    await open();
    let release: (() => void) | undefined;
    reorder.mockImplementationOnce(
      () =>
        new Promise((ok) => {
          release = () => ok([codexQuick, orchestrator]);
        }),
    );
    await clickPicker();
    trigger().focus();
    const ev = key(trigger(), "ArrowDown", { altKey: true });
    await tick();
    flushSync();
    expect(ev.defaultPrevented).toBe(true);
    expect(reorder).toHaveBeenCalledWith(["p2", "p1"]);
    expect(optionNames()).toEqual(["Codex quick", "Orchestrator"]);
    expect(live()).toBe("Orchestrator moved to position 2 of 2");
    expect(selectedCard()).toBe("p1");

    release?.();
    await tick();
    expect(reorder).toHaveBeenCalledTimes(1);
  });

  it("reverts the optimistic order when the reorder call fails", async () => {
    await open();
    let release: (() => void) | undefined;
    reorder.mockImplementationOnce(
      () =>
        new Promise((_ok, fail) => {
          release = () => fail(new Error("nope"));
        }),
    );
    await clickPicker();
    trigger().focus();
    key(trigger(), "ArrowDown", { altKey: true });
    await tick();
    flushSync();
    expect(optionNames()).toEqual(["Codex quick", "Orchestrator"]);

    release?.();
    await tick();
    flushSync();
    expect(optionNames()).toEqual(["Orchestrator", "Codex quick"]);
  });

  it("offers no reorder handle when there is a single profile", async () => {
    profileList = [orchestrator];
    await open();
    await clickPicker();
    expect(optionNames()).toEqual(["Orchestrator"]);
    expect(panel().querySelector(".rs-grip")).toBeNull();
  });
});
