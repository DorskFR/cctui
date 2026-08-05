import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";

/** An enrolled dispatcher: a standalone executor service enrolled per
 *  account that dials out over `/api/v1/dispatcher/ws`. Identity record only —
 *  the enrollment key is shown once at enroll time and never echoed here. */
export interface UserDispatcher {
  id: string;
  name: string;
  /** Reported by the binary at enroll: `kubernetes` | `docker` | `http`. */
  kind: string;
  /** Non-secret fragment of the enrollment key, for display. */
  key_preview: string | null;
  /** Liveness tier derived from `last_seen_at`: `online` | `stale` | `offline`. */
  liveness: string;
  /** Whether a live WS connection is currently registered. */
  connected: boolean;
  last_seen_at: string;
  created_at: string;
  updated_at: string;
}

/** Rename payload for an enrolled dispatcher. */
export interface RenameDispatcher {
  name: string;
}

/** Response to a dispatcher enroll — `dispatcher_key` is shown ONCE. */
export interface EnrollDispatcherResponse {
  dispatcher_id: string;
  dispatcher_key: string;
  server_version: string;
}

export const useDispatchers = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: ["dispatchers"],
    queryFn: endpoints.dispatchers,
    enabled: enabled(),
    staleTime: 60_000,
  }));

export const useUserDispatchers = () =>
  createQuery(() => ({
    queryKey: ["user-dispatchers"],
    queryFn: endpoints.userDispatchers,
  }));

/** Enroll / rename / remove the caller's enrolled dispatchers.
 *  Invalidates both the management list and the merged dispatch picker. */
export function useDispatcherActions() {
  const qc = useQueryClient();
  const inval = () => {
    qc.invalidateQueries({ queryKey: ["user-dispatchers"] });
    qc.invalidateQueries({ queryKey: ["dispatchers"] });
  };
  return {
    enroll: async (body: { name: string; kind?: string; account?: string; provider?: string }) => {
      const r = await endpoints.enrollDispatcher(body);
      inval();
      return r;
    },
    rename: async (id: string, body: RenameDispatcher) => {
      const r = await endpoints.updateDispatcher(id, body);
      inval();
      return r;
    },
    remove: async (id: string) => {
      await endpoints.deleteDispatcher(id);
      inval();
    },
  };
}
