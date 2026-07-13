import { closeMergedPr } from "./merge-transition";
import {
  closeTab,
  defaultStatus,
  deserialize,
  openTab,
  type PrTab,
  serialize,
  setActive,
  type TabStatus,
  type TabsState,
  tabId,
  updateStatus,
} from "./tabs-core";

const STORAGE_KEY = "ghreview:tabs";

class TabsStore {
  state = $state<TabsState>(deserialize(localStorage.getItem(STORAGE_KEY)));

  get tabs(): PrTab[] {
    return this.state.tabs;
  }

  get activeId(): string | null {
    return this.state.activeId;
  }

  private persist(): void {
    localStorage.setItem(STORAGE_KEY, serialize(this.state));
  }

  open(owner: string, repo: string, number: number, title: string): string {
    const id = tabId(owner, repo, number);
    this.state = openTab(this.state, {
      id,
      owner,
      repo,
      number,
      title,
      status: defaultStatus(),
    });
    this.persist();
    return id;
  }

  close(id: string): void {
    this.state = closeTab(this.state, id);
    this.persist();
  }

  closeMerged(owner: string, repo: string, number: number): string {
    const transition = closeMergedPr(this.state, owner, repo, number);
    this.state = transition.state;
    this.persist();
    return transition.path;
  }

  activate(id: string | null): void {
    this.state = setActive(this.state, id);
    this.persist();
  }

  setStatus(id: string, status: Partial<TabStatus>): void {
    this.state = updateStatus(this.state, id, status);
    this.persist();
  }

  find(owner: string, repo: string, number: number): PrTab | undefined {
    const id = tabId(owner, repo, number);
    return this.state.tabs.find((t) => t.id === id);
  }
}

export const tabs = new TabsStore();
