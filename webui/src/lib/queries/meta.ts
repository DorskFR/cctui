import { createQuery } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

export const useMe = () =>
  createQuery({
    queryKey: ["me"],
    queryFn: endpoints.me,
    staleTime: 5 * 60_000,
  });

/** Server capability flags. Long stale time — capabilities only
 * change on install/uninstall, which is rare and owner-driven. */
export const useCapabilities = () =>
  createQuery({
    queryKey: qk.capabilities,
    queryFn: endpoints.capabilities,
    staleTime: 5 * 60_000,
  });

/** The settings catalog. Embedded server data — effectively
 * immutable per server version, so cache it for the whole session. */
export const useSettingsCatalog = () =>
  createQuery({
    queryKey: qk.settingsCatalog,
    queryFn: endpoints.settingsCatalog,
    staleTime: Infinity,
  });

export const useVersion = () =>
  createQuery({
    queryKey: qk.version,
    queryFn: endpoints.version,
    staleTime: 60_000,
  });
