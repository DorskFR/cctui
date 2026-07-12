import {
  deserialize,
  type LayoutMode,
  type LayoutState,
  serialize,
  setMode,
  toggleFullWidth,
  toggleMode,
} from "./layout-core";

const STORAGE_KEY = "ghreview:layout";

class LayoutStore {
  state = $state<LayoutState>(deserialize(localStorage.getItem(STORAGE_KEY)));

  get mode(): LayoutMode {
    return this.state.mode;
  }

  get fullWidth(): boolean {
    return this.state.fullWidth;
  }

  private persist(): void {
    localStorage.setItem(STORAGE_KEY, serialize(this.state));
  }

  toggleMode(): void {
    this.state = toggleMode(this.state);
    this.persist();
  }

  setMode(mode: LayoutMode): void {
    this.state = setMode(this.state, mode);
    this.persist();
  }

  toggleFullWidth(): void {
    this.state = toggleFullWidth(this.state);
    this.persist();
  }
}

export const layout = new LayoutStore();
