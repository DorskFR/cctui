import type { CiState, PrState } from "../api/types";

export interface TabStatus {
  pr: PrState;
  ci: CiState;
  mergeable: boolean | null;
}

export interface PrTab {
  id: string;
  owner: string;
  repo: string;
  number: number;
  title: string;
  status: TabStatus;
}

export interface TabsState {
  tabs: PrTab[];
  activeId: string | null;
}

export function tabId(owner: string, repo: string, number: number): string {
  return `pr-${owner}-${repo}-${number}`;
}

export function defaultStatus(): TabStatus {
  return { pr: "open", ci: "none", mergeable: null };
}

export function openTab(state: TabsState, tab: PrTab): TabsState {
  const existing = state.tabs.find((t) => t.id === tab.id);
  if (existing) {
    if (existing.title === tab.title && state.activeId === tab.id) return state;
    return {
      tabs: state.tabs.map((t) => (t.id === tab.id ? { ...t, ...tab, status: t.status } : t)),
      activeId: tab.id,
    };
  }
  return { tabs: [...state.tabs, tab], activeId: tab.id };
}

export function closeTab(state: TabsState, id: string): TabsState {
  const index = state.tabs.findIndex((t) => t.id === id);
  if (index < 0) return state;
  const tabs = state.tabs.filter((t) => t.id !== id);
  let activeId = state.activeId;
  if (state.activeId === id) {
    if (tabs.length === 0) activeId = null;
    else activeId = tabs[Math.min(index, tabs.length - 1)].id;
  }
  return { tabs, activeId };
}

export function setActive(state: TabsState, id: string | null): TabsState {
  if (id !== null && !state.tabs.some((t) => t.id === id)) return state;
  if (state.activeId === id) return state;
  return { ...state, activeId: id };
}

export function updateStatus(state: TabsState, id: string, status: Partial<TabStatus>): TabsState {
  const target = state.tabs.find((t) => t.id === id);
  if (!target) return state;
  const merged = { ...target.status, ...status };
  if (
    merged.pr === target.status.pr &&
    merged.ci === target.status.ci &&
    merged.mergeable === target.status.mergeable
  ) {
    return state;
  }
  return {
    ...state,
    tabs: state.tabs.map((t) => (t.id === id ? { ...t, status: merged } : t)),
  };
}

export function serialize(state: TabsState): string {
  return JSON.stringify(state);
}

export function deserialize(raw: string | null): TabsState {
  const empty: TabsState = { tabs: [], activeId: null };
  if (!raw) return empty;
  try {
    const parsed = JSON.parse(raw) as Partial<TabsState>;
    if (!parsed || !Array.isArray(parsed.tabs)) return empty;
    const tabs = parsed.tabs.filter(
      (t): t is PrTab =>
        !!t &&
        typeof t.id === "string" &&
        typeof t.owner === "string" &&
        typeof t.repo === "string" &&
        typeof t.number === "number",
    );
    const activeId =
      typeof parsed.activeId === "string" && tabs.some((t) => t.id === parsed.activeId)
        ? parsed.activeId
        : (tabs.at(-1)?.id ?? null);
    return { tabs, activeId };
  } catch {
    return empty;
  }
}
