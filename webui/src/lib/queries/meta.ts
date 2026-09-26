import { createQuery } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

export const useMe = () =>
  createQuery(() => ({
    queryKey: qk.me,
    queryFn: endpoints.me,
    staleTime: 5 * 60_000,
  }));

/** Server capability flags. Long stale time — capabilities only
 * change on install/uninstall, which is rare and owner-driven. */
export const useCapabilities = () =>
  createQuery(() => ({
    queryKey: qk.capabilities,
    queryFn: endpoints.capabilities,
    staleTime: 5 * 60_000,
  }));

/** The settings catalog. Embedded server data — effectively
 * immutable per server version, so cache it for the whole session. */
export const useSettingsCatalog = (family: () => string = () => "anthropic") =>
  createQuery(() => ({
    queryKey: qk.settingsCatalog(family()),
    queryFn: () => endpoints.settingsCatalog(family()),
    staleTime: Infinity,
  }));

export const useVersion = () =>
  createQuery(() => ({
    queryKey: qk.version,
    queryFn: endpoints.version,
    staleTime: 60_000,
  }));

/** Release notes for `version`, as collected by the server's update probe. */
export const useChangelog = (version: () => string) =>
  createQuery(() => ({
    queryKey: qk.changelog(version()),
    queryFn: endpoints.changelog,
    staleTime: 60_000,
  }));

/** The in-flight self-update hook run; polls until it reports a terminal phase. */
export const useSelfUpdateRun = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.selfUpdateRun,
    queryFn: endpoints.selfUpdateStatus,
    enabled: enabled(),
    refetchInterval: (query) => (query.state.data?.done ? false : 3_000),
  }));
