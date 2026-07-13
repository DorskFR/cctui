import {
  deserialize,
  type LayoutState,
  serialize,
  setSidebarCollapsed,
  toggleSidebar,
} from "./layout-core";

const STORAGE_KEY = "ghreview:layout";

class LayoutStore {
  state = $state<LayoutState>(deserialize(localStorage.getItem(STORAGE_KEY)));

  get sidebarCollapsed(): boolean {
    return this.state.sidebarCollapsed;
  }

  private persist(): void {
    localStorage.setItem(STORAGE_KEY, serialize(this.state));
  }

  toggleSidebar(): void {
    this.state = toggleSidebar(this.state);
    this.persist();
  }

  setSidebarCollapsed(collapsed: boolean): void {
    this.state = setSidebarCollapsed(this.state, collapsed);
    this.persist();
  }
}

export const layout = new LayoutStore();
