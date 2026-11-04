// @vitest-environment happy-dom
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SvelteMap } from "svelte/reactivity";
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
const personal = {
  id: "a1",
  name: "personal",
  emoji: "🐼",
  providers: [
    { id: "pr1", provider: "anthropic", models: [], model_aliases: null },
  ],
};
let accounts: Record<string, unknown>[] = [];
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
  name: "Quick codex",
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
const remove = vi.fn();
// Profiles the server holds after a delete; the derived profile list tracks it.
const live = new SvelteMap<"list", SessionProfile[]>();

vi.mock("$lib/queries", () => {
  const q = <T>(data: T) => ({ data, isLoading: false, isError: false });
  return {
    useMachineResources: () => q(machineList),
    useVoiceConfig: () => q(undefined),
    useDispatchers: () => q([]),
    useSessions: () => q({ sessions: [] }),
    useRecentDirs: () => q([]),
    useAccounts: () => ({ data: accounts, isLoading: false, isError: false }),
    useAccountPools: () => q([{ id: "pool1", name: "shared", members: [] }]),
    useLabels: () => q({ labels: [] }),
    useContextItems: () => q([]),
    useResolvedContext: () => q({ rules: [], memories: [] }),
    useProfiles: () => ({
      get data() {
        return live.get("list") ?? profileList;
      },
      isLoading: false,
      isError: false,
    }),
    useProfileActions: () => ({ create, update, remove }),
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
  // Stored order is the reverse of A–Z: the picker must ignore it.
  profileList = [codexQuick, orchestrator];
  accounts = [personal];
  live.clear();
  lastEntry = null;
  spawn
    .mockReset()
    .mockResolvedValue({ command_id: "c-1", status: "ok", account: null });
  create
    .mockReset()
    .mockResolvedValue({ ...orchestrator, id: "p-new", name: "Default" });
  update.mockReset().mockResolvedValue(orchestrator);
  remove.mockReset().mockImplementation(async (id: string) => {
    live.set(
      "list",
      (live.get("list") ?? profileList).filter((p) => p.id !== id),
    );
  });
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
    '[data-journey="profiles"] [data-journey="profile-trigger"]',
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
  options().map((o) => o.querySelector(".name")?.textContent?.trim());
const badge = (root: Element | null | undefined) =>
  root?.querySelector<HTMLElement>('[data-journey="account-badge"]') ?? null;
const cog = () => button("Adjust profile");
const profileForm = () =>
  document.querySelector<HTMLElement>('[data-journey="profile-form"]');
const nameInput = () =>
  profileForm()?.querySelector<HTMLInputElement>('input[id^="sp-profile-name"]') ??
  null;
async function typeName(v: string) {
  const el = nameInput();
  if (!el) throw new Error("profile name input not found");
  el.value = v;
  el.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
  flushSync();
}
async function pickAccount(value: string) {
  const select = profileForm()?.querySelector<HTMLSelectElement>(
    'select[id^="sp-kit-account"]',
  );
  if (!select) throw new Error("account select not found");
  const pick = [...select.options].find((o) => o.value === value);
  if (!pick) throw new Error(`account option ${value} not found`);
  pick.selected = true;
  // happy-dom cannot match `option:checked`, which Svelte's select binding reads.
  const nativeQuery = select.querySelector.bind(select);
  select.querySelector = ((q: string) =>
    q === ":checked" ? pick : nativeQuery(q)) as typeof select.querySelector;
  select.dispatchEvent(new Event("change", { bubbles: true }));
  await tick();
  flushSync();
}
async function openNewProfile() {
  await clickPicker();
  const footer = panel().querySelector<HTMLButtonElement>(
    '[data-journey="new-profile"]',
  );
  if (!footer) throw new Error("footer button not found");
  footer.click();
  await tick();
  flushSync();
}
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
async function submit() {
  spawnButton().click();
  await tick(80);
  expect(spawn).toHaveBeenCalledTimes(1);
  return spawn.mock.calls[0][0];
}

describe("SpawnModal profiles", () => {
  it("closed, it shows the selected profile as one card with its account badge and the cog inside", async () => {
    await open();
    expect(selectedCard()).toBe("p1");
    expect(cards()).toHaveLength(1);
    expect(options()).toHaveLength(0);
    expect(expanded()).toBe(false);
    expect(trigger().getAttribute("aria-haspopup")).toBe("listbox");
    expect(trigger().textContent).toContain(
      "Claude Code · personal · Fable · medium · Yolo",
    );
    const b = badge(trigger());
    expect(b?.textContent?.trim()).toBe("🐼");
    expect(b?.getAttribute("aria-hidden")).toBe("true");
    expect(b?.dataset.kind).toBe("account");
    const card = document.querySelector('[data-journey="profiles"]')
      ?.firstElementChild;
    expect(card?.contains(cog())).toBe(true);
    expect(trigger().contains(cog())).toBe(false);
  });

  it("never shows a usage hint, closed or open", async () => {
    localStorage.setItem(
      "cctui_profile_uses",
      JSON.stringify({ p1: [Date.now() - 1000], p2: [Date.now() - 1000] }),
    );
    await open();
    expect(trigger().textContent).not.toMatch(/this week|today|ago/);
    await clickPicker();
    for (const o of options())
      expect(o.textContent).not.toMatch(/this week|today|ago/);
  });

  it("opens an A–Z list (not the stored order) of badge rows with no grip, cog or tick, and a New profile footer", async () => {
    await open();
    await clickPicker();
    expect(expanded()).toBe(true);
    expect(optionNames()).toEqual(["Orchestrator", "Quick codex"]);
    expect(option("p1").getAttribute("aria-selected")).toBe("true");
    expect(option("p2").getAttribute("aria-selected")).toBe("false");
    const rows = options();
    expect(rows).toHaveLength(2);
    for (const row of rows) {
      expect(row.querySelectorAll('[data-journey="account-badge"]')).toHaveLength(1);
      expect(row.querySelector("button")).toBeNull();
    }
    expect(panel().querySelectorAll("button")).toHaveLength(1);
    const footer = panel().querySelector('[data-journey="new-profile"]');
    expect(footer?.textContent?.trim()).toBe("New profile");
    expect(
      panel().querySelector('[role="listbox"]')?.contains(footer ?? null),
    ).toBe(false);
  });

  it("badges an account by emoji or initial, a pool by its initial and no account with ?", async () => {
    accounts = [personal, { ...personal, id: "a2", name: "work", emoji: null }];
    profileList = [
      orchestrator,
      { ...codexQuick, id: "p2", name: "B work", account_id: "a2" },
      { ...codexQuick, id: "p3", name: "C pool", pool_id: "pool1" },
      { ...codexQuick, id: "p4", name: "D none", no_account: true },
      { ...codexQuick, id: "p5", name: "E auto" },
    ];
    await open();
    await clickPicker();
    const look = (id: string) => {
      const b = badge(option(id));
      return [b?.textContent?.trim(), b?.dataset.kind];
    };
    expect(look("p1")).toEqual(["🐼", "account"]);
    expect(look("p2")).toEqual(["W", "account"]);
    expect(look("p3")).toEqual(["S", "pool"]);
    expect(look("p4")).toEqual(["?", "none"]);
    expect(look("p5")).toEqual(["?", "none"]);
    expect(badge(option("p1"))?.style.getPropertyValue("--mh")).not.toBe("");
    expect(badge(option("p4"))?.style.getPropertyValue("--mh")).toBe("");
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

  it("arrow keys on the closed picker step through profiles in A–Z order without opening it", async () => {
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

  it("the cog toggles an edit form for the current profile; Escape and opening the list discard it", async () => {
    await open();
    cog().click();
    await tick();
    expect(profileForm()?.dataset.mode).toBe("edit");
    expect(profileForm()?.dataset.profileId).toBe("p1");
    expect(nameInput()?.value).toBe("Orchestrator");
    expect(cog().getAttribute("aria-expanded")).toBe("true");

    cog().click();
    await tick();
    expect(profileForm()).toBeNull();

    cog().click();
    await tick();
    key(nameInput() as HTMLInputElement, "Escape");
    await tick();
    flushSync();
    expect(profileForm()).toBeNull();

    cog().click();
    await tick();
    await clickPicker();
    expect(expanded()).toBe(true);
    expect(profileForm()).toBeNull();
  });

  it('"Use once" adjusts this run only and leaves the profile untouched', async () => {
    await open();
    cog().click();
    await tick();
    mode("ask").click();
    await tick();
    expect(document.body.textContent).toContain("1 changes");
    button("Use once").click();
    await tick();
    expect(update).not.toHaveBeenCalled();
    expect(profileForm()).toBeNull();
    expect(trigger().textContent).toContain("· Ask");
    const body = await submit();
    expect(body).toMatchObject({ permission_mode: "ask", model: "fable" });
  });

  it('"Save to profile" persists the adjusted kit', async () => {
    await open();
    cog().click();
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

  it("Delete removes the edited profile and falls back to the first remaining profile A–Z", async () => {
    const beta = { ...codexQuick, id: "p3", name: "Beta" };
    profileList = [codexQuick, beta, orchestrator];
    lastEntry = { profile_id: "p1" };
    await open();
    expect(selectedCard()).toBe("p1");
    cog().click();
    await tick();
    expect(() => button("Delete profile").click()).not.toThrow();
    await tick(100);
    flushSync();
    expect(remove).toHaveBeenCalledTimes(1);
    expect(remove).toHaveBeenCalledWith("p1");
    expect(profileForm()).toBeNull();
    expect(selectedCard()).toBe("p3");
  });

  it("New profile opens an empty create form and a draft card, and writes nothing until Create profile", async () => {
    await open();
    await openNewProfile();
    expect(
      document.querySelector('[data-journey="profile-trigger"]'),
    ).toBeNull();
    expect(options()).toHaveLength(0);
    expect(create).not.toHaveBeenCalled();
    expect(profileForm()?.dataset.mode).toBe("create");
    expect(nameInput()?.value).toBe("");
    const draft = document.querySelector<HTMLElement>(
      '[data-journey="profile-draft"]',
    );
    expect(draft?.textContent).toContain("New profile");
    expect(badge(draft)?.textContent?.trim()).toBe("🐼");

    const createBtn = button("Create profile");
    expect(createBtn.disabled).toBe(true);
    await typeName("  orchestrator ");
    expect(button("Create profile").disabled).toBe(true);
    await typeName("Reviewer");
    expect(button("Create profile").disabled).toBe(false);
    expect(create).not.toHaveBeenCalled();

    button("Create profile").click();
    await tick();
    flushSync();
    expect(create).toHaveBeenCalledTimes(1);
    expect(create.mock.calls[0][0]).toMatchObject({
      name: "Reviewer",
      harness: "claude-code",
      account_id: "a1",
      model_alias: "fable",
      permission_mode: "yolo",
    });
    expect(profileForm()).toBeNull();
  });

  it("the draft card's badge follows the Account field live", async () => {
    await open();
    await openNewProfile();
    const draftBadge = () =>
      badge(document.querySelector('[data-journey="profile-draft"]'));
    expect(draftBadge()?.textContent?.trim()).toBe("🐼");
    await pickAccount("\x00pool:pool1");
    expect(draftBadge()?.textContent?.trim()).toBe("S");
    await pickAccount("\x00no-account");
    expect(draftBadge()?.textContent?.trim()).toBe("?");
  });

  it("Cancel on the create form writes nothing and returns to the previous selection", async () => {
    await open();
    await openNewProfile();
    await typeName("Reviewer");
    button("Cancel").click();
    await tick();
    flushSync();
    expect(create).not.toHaveBeenCalled();
    expect(profileForm()).toBeNull();
    expect(selectedCard()).toBe("p1");
  });

  it('"Use once" on the create form spawns with the draft without saving it', async () => {
    await open();
    await openNewProfile();
    mode("ask").click();
    await tick();
    button("Use once").click();
    await tick();
    flushSync();
    expect(create).not.toHaveBeenCalled();
    expect(profileForm()).toBeNull();
    const body = await submit();
    expect(body).toMatchObject({ permission_mode: "ask", model: "fable" });
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

  it('offers pools and "no account" in the edit form and persists the pick', async () => {
    await open();
    cog().click();
    await tick();
    const select = document.querySelector<HTMLSelectElement>(
      'select[id^="sp-kit-account"]',
    );
    if (!select) throw new Error("account select not found");
    const values = [...select.options].map((o) => o.value);
    expect(values).toEqual(["\x00pool:pool1", "a1", "", "\x00no-account"]);
    await pickAccount("\x00pool:pool1");
    button("Save to profile").click();
    await tick();
    expect(update.mock.calls[0][1]).toMatchObject({
      spec: { account_id: null, pool_id: "pool1", no_account: false },
    });
  });

  it("with no profile it renders the create form on the one-off kit and settles (no effect loop)", async () => {
    profileList = [];
    await open();
    await tick(120);
    // An unguarded mirror of `oneOff` re-entered until Svelte threw
    // effect_update_depth_exceeded and the tab died.
    expect(profileForm()?.dataset.mode).toBe("create");
    expect(button("Create profile")).toBeTruthy();
    expect(() => button("Cancel")).toThrow();
    expect(mode("ask")).toBeTruthy();
    expect(
      document.querySelector('select[id^="sp-kit-account"]'),
    ).not.toBeNull();
    mode("yolo").click();
    await tick();
    expect(mode("yolo").getAttribute("aria-checked")).toBe("true");
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
