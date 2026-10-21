import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { api } from "../api";
import type { DispatchResponse } from "@bindings/DispatchResponse";
import type { FailedDispatch } from "@bindings/FailedDispatch";

export type { FailedDispatch } from "@bindings/FailedDispatch";

const key = ["dispatch-records", "failed"] as const;

export const failedDispatchEndpoints = {
  list: () => api.get<FailedDispatch[]>("/dispatch-records/failed"),
  redispatch: (sessionId: string) =>
    api.post<DispatchResponse>(
      `/dispatch-records/${encodeURIComponent(sessionId)}/redispatch`,
      {},
    ),
};

export const useFailedDispatches = () =>
  createQuery(() => ({
    queryKey: key,
    queryFn: failedDispatchEndpoints.list,
    refetchInterval: 60_000,
  }));

export function useRedispatch() {
  const qc = useQueryClient();
  return async (sessionId: string) => {
    const r = await failedDispatchEndpoints.redispatch(sessionId);
    await qc.invalidateQueries({ queryKey: key });
    return r;
  };
}
