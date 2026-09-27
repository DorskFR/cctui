import { createQuery } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

/** Installed runtime plugins; empty when the server has no plugins dir. */
export const usePlugins = () =>
  createQuery(() => ({
    queryKey: qk.plugins,
    queryFn: () => endpoints.plugins(),
    staleTime: 60_000,
  }));

/** Every plugin, including instance-disabled ones (admin). */
export const useAdminPlugins = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.adminPlugins,
    queryFn: () => endpoints.adminPlugins(),
    enabled: enabled(),
  }));
