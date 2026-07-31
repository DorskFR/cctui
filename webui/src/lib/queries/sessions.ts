import { createQuery } from "@tanstack/svelte-query";
import { toStore } from "svelte/store";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

/* This svelte-query build types options as `T | Readable<T>` (not an accessor
 * function), so reactive params are bridged from runes via Svelte 5's
 * `toStore(getter)`; param-less queries pass a plain options object. */

export const useSessions = (
  archived: () => boolean,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.sessions(archived()),
      queryFn: () => endpoints.sessions(archived()),
      refetchInterval: 15_000,
      enabled: enabled(),
    })),
  );

export const useSessionStats = () =>
  createQuery({
    queryKey: qk.sessionStats,
    queryFn: endpoints.sessionStats,
    refetchInterval: 15_000,
  });

/** All label definitions. Shared by the per-session picker and the
 * sessions-page filter; refetched lazily since labels change rarely. */
export const useLabels = () =>
  createQuery({
    queryKey: qk.labels,
    queryFn: endpoints.labels,
    refetchInterval: 60_000,
  });

export const useTokenStats = () =>
  createQuery({
    queryKey: qk.tokenStats,
    // Resolve the offset per fetch so it stays correct across a DST change.
    queryFn: () => endpoints.tokenStats(new Date().getTimezoneOffset()),
    refetchInterval: 15_000,
  });

export const useUsageAnalytics = (days: () => number) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.usageAnalytics(days()),
      queryFn: () =>
        endpoints.usageAnalytics(days(), new Date().getTimezoneOffset()),
      refetchInterval: 60_000,
    })),
  );

/** Older pages (`before` cursor) deliberately bypass the query cache. */
export const CONVERSATION_FETCH_LIMIT = 60;

export const useConversation = (
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.conversation(id()),
      queryFn: () =>
        endpoints.conversation(id(), { limit: CONVERSATION_FETCH_LIMIT }),
      enabled: enabled() && !!id(),
    })),
  );

/** Session diagnose panel. Fetched only while the panel is open;
 *  no polling — the panel offers an explicit refresh instead, since the call
 *  round-trips through the daemon. */
export const useSessionDiagnose = (
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: ["session-diagnose", id()],
      queryFn: () => endpoints.sessionDiagnose(id()),
      enabled: enabled() && !!id(),
      staleTime: 0,
      retry: false,
    })),
  );

/** Per-session Langfuse cost/usage chip. Lazy — fetched only while
 *  the drawer is open and the capability is present; the server caches ~60s so
 *  a short client stale time won't hammer upstream. Fail-open: on error the
 *  chip simply hides. */
export const useSessionLangfuse = (
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: ["session-langfuse", id()],
      queryFn: () => endpoints.sessionLangfuse(id()),
      enabled: enabled() && !!id(),
      staleTime: 60_000,
      retry: false,
    })),
  );

export const useRecentDirs = (machineId: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: ["recent-dirs", machineId()],
      queryFn: () => endpoints.recentDirs(machineId()),
      enabled: !!machineId(),
      staleTime: 30_000,
    })),
  );

export const useMachineDirs = (machineId: () => string, path: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: ["machine-dirs", machineId(), path()],
      queryFn: () => endpoints.machineDirs(machineId(), path()),
      enabled: !!machineId() && !!path(),
      staleTime: 10_000,
      retry: false,
    })),
  );

export const useCodexModels = (machineId: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: ["codex-models", machineId()],
      queryFn: () => endpoints.codexModels(machineId()),
      enabled: !!machineId(),
      staleTime: 60_000,
      retry: false,
    })),
  );

export const useSessionBindings = (sessionId: () => string, enabled: () => boolean = () => true) =>
  createQuery(
    toStore(() => ({
      queryKey: ["session-bindings", sessionId()],
      queryFn: () => endpoints.sessionBindings(sessionId()),
      enabled: enabled(),
    })),
  );
