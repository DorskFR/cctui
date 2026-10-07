import { createQuery } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";
import { setDomainMeta } from "$lib/domainMeta.svelte";
import { setHarnesses } from "$lib/harnesses.svelte";

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

/** The server-owned half of the domain metadata — today the quota-probe
 *  registry. Constant per server version, so cache it for the whole session.
 *  The closed tables are `$lib/domainTables`, not this. */
export const useDomainMeta = () =>
  createQuery(() => ({
    queryKey: qk.domainMeta,
    queryFn: async () => {
      const meta = await endpoints.domainMeta();
      setDomainMeta(meta);
      return meta;
    },
    staleTime: Infinity,
  }));

/** The harness table. Constant per server version; the shipped copy answers
 *  until this lands, then the served rows take over. */
export const useHarnesses = () =>
  createQuery(() => ({
    queryKey: qk.harnesses,
    queryFn: async () => {
      const rows = await endpoints.harnesses();
      setHarnesses(rows);
      return rows;
    },
    staleTime: Infinity,
  }));
