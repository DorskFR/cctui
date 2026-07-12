import type { ViewedStateItem, ViewedStateResult } from "./types";

export function viewedSet(result: ViewedStateResult | undefined): Set<string> {
  const set = new Set<string>();
  for (const item of result?.items ?? []) {
    if (item.viewed) set.add(item.path);
  }
  return set;
}

export function applyOptimisticViewed(
  current: ViewedStateResult | undefined,
  paths: string[],
  viewed: boolean,
): ViewedStateResult {
  const byPath = new Map<string, ViewedStateItem>();
  for (const item of current?.items ?? []) byPath.set(item.path, item);
  for (const path of paths) {
    const prev = byPath.get(path);
    byPath.set(path, {
      path,
      viewed,
      digest: prev?.digest ?? null,
      push_pending: true,
      last_error: null,
      updated_at: prev?.updated_at ?? null,
    });
  }
  return { items: [...byPath.values()].sort((a, b) => a.path.localeCompare(b.path)) };
}
