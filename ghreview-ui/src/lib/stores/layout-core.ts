export interface LayoutState {
  sidebarCollapsed: boolean;
}

export function defaultLayout(): LayoutState {
  return { sidebarCollapsed: false };
}

export function toggleSidebar(state: LayoutState): LayoutState {
  return { ...state, sidebarCollapsed: !state.sidebarCollapsed };
}

export function setSidebarCollapsed(state: LayoutState, collapsed: boolean): LayoutState {
  return { ...state, sidebarCollapsed: collapsed };
}

export function serialize(state: LayoutState): string {
  return JSON.stringify(state);
}

export function deserialize(raw: string | null): LayoutState {
  if (!raw) return defaultLayout();
  try {
    const parsed = JSON.parse(raw) as Partial<LayoutState>;
    return { sidebarCollapsed: parsed?.sidebarCollapsed === true };
  } catch {
    return defaultLayout();
  }
}
