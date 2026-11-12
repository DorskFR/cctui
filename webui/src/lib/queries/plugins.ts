import { createQuery } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";
import { hasFeature } from "$lib/features.svelte";

/** Installed runtime plugins; empty when the server has no plugins dir. */
export const usePlugins = () =>
  createQuery(() => ({
    queryKey: qk.plugins,
    queryFn: () => endpoints.plugins(),
    enabled: hasFeature("plugins"),
    staleTime: 60_000,
  }));

/** Every plugin, including instance-disabled ones (admin). */
export const useAdminPlugins = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.adminPlugins,
    queryFn: () => endpoints.adminPlugins(),
    enabled: enabled() && hasFeature("plugins"),
  }));

/** The published catalog, annotated with installed versions (admin). */
export const useAdminPluginCatalog = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.adminPluginCatalog,
    queryFn: () => endpoints.adminPluginCatalog(),
    enabled: enabled() && hasFeature("plugins"),
    staleTime: 5 * 60_000,
  }));

/** One plugin's instance-level settings; only fetched while its form is open. */
export const usePluginInstanceSettings = (id: () => string | null) =>
  createQuery(() => ({
    queryKey: qk.adminPluginSettings(id() ?? ""),
    queryFn: () => endpoints.pluginInstanceSettings(id()!),
    enabled: id() !== null && hasFeature("plugins"),
  }));
