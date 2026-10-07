import { createQuery } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

export const EVENT_PAGE = 50;
export const SESSION_EVENT_PAGE = 100;
export const MACHINE_HISTORY_PAGE = 30;

/** First page of the feed for one filter set; later pages are fetched by the
 *  feed itself on the cursor and the socket prepends live rows into this key. */
export const useEvents = (query: () => Record<string, string>, enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.events(query()),
    queryFn: () => endpoints.events({ ...query(), limit: String(EVENT_PAGE) }),
    enabled: enabled(),
    staleTime: 30_000,
  }));

export const useSessionEvents = (id: () => string, enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.sessionEvents(id()),
    queryFn: () => endpoints.sessionEvents(id(), { limit: String(SESSION_EVENT_PAGE) }),
    enabled: enabled() && !!id(),
    staleTime: 30_000,
  }));

export const useMachineEvents = (machineId: () => string, enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.machineEvents(machineId()),
    queryFn: () =>
      endpoints.machineEvents(machineId(), {
        kind: "machine.",
        limit: String(MACHINE_HISTORY_PAGE),
      }),
    enabled: enabled() && !!machineId(),
    staleTime: 30_000,
  }));
