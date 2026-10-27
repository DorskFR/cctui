import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { api } from "../api";
import type { DismissedDispatches } from "@bindings/DismissedDispatches";
import type { DispatchResponse } from "@bindings/DispatchResponse";
import type { FailedDispatch } from "@bindings/FailedDispatch";

export type { FailedDispatch } from "@bindings/FailedDispatch";

const key = ["dispatch-records", "failed"] as const;
const historyKey = [...key, "history"] as const;

export const RECENT_FAILURE_HOURS = 24;

export const failedDispatchEndpoints = {
  list: () => api.get<FailedDispatch[]>("/dispatch-records/failed"),
  history: () =>
    api.get<FailedDispatch[]>("/dispatch-records/failed?history=true"),
  redispatch: (sessionId: string) =>
    api.post<DispatchResponse>(
      `/dispatch-records/${encodeURIComponent(sessionId)}/redispatch`,
      {},
    ),
  dismiss: (sessionId: string) =>
    api.post<void>(
      `/dispatch-records/${encodeURIComponent(sessionId)}/dismiss`,
      {},
    ),
  dismissAll: () =>
    api.post<DismissedDispatches>("/dispatch-records/dismiss-all", {}),
};

export type FailureGroups = {
  active: FailedDispatch[];
  resolved: FailedDispatch[];
  older: FailedDispatch[];
};

export function groupFailures(
  list: FailedDispatch[],
  now: number = Date.now(),
  recentHours: number = RECENT_FAILURE_HOURS,
): FailureGroups {
  const since = now - recentHours * 3_600_000;
  const groups: FailureGroups = { active: [], resolved: [], older: [] };
  for (const d of list) {
    if (d.dismissed) continue;
    const recent = new Date(d.dispatched_at).getTime() >= since;
    if (d.superseded_by) {
      if (recent) groups.resolved.push(d);
    } else {
      (recent ? groups.active : groups.older).push(d);
    }
  }
  return groups;
}

export const useFailedDispatches = () =>
  createQuery(() => ({
    queryKey: key,
    queryFn: failedDispatchEndpoints.list,
    refetchInterval: 60_000,
  }));

export const useFailedDispatchHistory = () =>
  createQuery(() => ({
    queryKey: historyKey,
    queryFn: failedDispatchEndpoints.history,
  }));

export function useRedispatch() {
  const qc = useQueryClient();
  return async (sessionId: string) => {
    const r = await failedDispatchEndpoints.redispatch(sessionId);
    await qc.invalidateQueries({ queryKey: key });
    return r;
  };
}

export function useDismissFailedDispatch() {
  const qc = useQueryClient();
  return async (sessionId: string) => {
    qc.setQueryData<FailedDispatch[]>(key, (list) =>
      list?.filter((d) => d.session_id !== sessionId),
    );
    try {
      await failedDispatchEndpoints.dismiss(sessionId);
    } finally {
      await qc.invalidateQueries({ queryKey: key });
    }
  };
}

export function useDismissAllFailedDispatches() {
  const qc = useQueryClient();
  return async () => {
    try {
      return await failedDispatchEndpoints.dismissAll();
    } finally {
      await qc.invalidateQueries({ queryKey: key });
    }
  };
}
