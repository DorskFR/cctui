import { pullPath } from "../router/route";
import { closeTab, type TabsState, tabId } from "./tabs-core";

export interface MergeTransition {
  state: TabsState;
  path: string;
}

export function closeMergedPr(
  state: TabsState,
  owner: string,
  repo: string,
  number: number,
): MergeTransition {
  const nextState = closeTab(state, tabId(owner, repo, number));
  const next = nextState.tabs.find((tab) => tab.id === nextState.activeId);
  return {
    state: nextState,
    path: next ? pullPath(next.owner, next.repo, next.number) : "/",
  };
}
