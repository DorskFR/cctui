export type LayoutMode = "panels" | "tabs";

export interface LayoutState {
  mode: LayoutMode;
  fullWidth: boolean;
}

export function defaultLayout(): LayoutState {
  return { mode: "panels", fullWidth: false };
}

export function toggleMode(state: LayoutState): LayoutState {
  return { ...state, mode: state.mode === "panels" ? "tabs" : "panels" };
}

export function setMode(state: LayoutState, mode: LayoutMode): LayoutState {
  return { ...state, mode };
}

export function toggleFullWidth(state: LayoutState): LayoutState {
  return { ...state, fullWidth: !state.fullWidth };
}

export function serialize(state: LayoutState): string {
  return JSON.stringify(state);
}

export function deserialize(raw: string | null): LayoutState {
  if (!raw) return defaultLayout();
  try {
    const parsed = JSON.parse(raw) as Partial<LayoutState>;
    const mode: LayoutMode = parsed?.mode === "tabs" ? "tabs" : "panels";
    const fullWidth = parsed?.fullWidth === true;
    return { mode, fullWidth };
  } catch {
    return defaultLayout();
  }
}
