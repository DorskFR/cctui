import { type QueryClient, createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { StorageLifecycleInfo } from "@bindings/StorageLifecycleInfo";
import type { StorageLifecyclePreview } from "@bindings/StorageLifecyclePreview";
import type { StorageLifecycleRules } from "@bindings/StorageLifecycleRules";
import type { StorageOverview } from "@bindings/StorageOverview";
import { api } from "../api";
import { qk } from "./keys";

export const storageAdminApi = {
  overview: () => api.get<StorageOverview>("/admin/storage"),
  setLifecycle: (rules: StorageLifecycleRules) =>
    api.put<StorageLifecycleInfo>("/admin/storage/lifecycle", rules),
  preview: (rules: StorageLifecycleRules) =>
    api.post<StorageLifecyclePreview>("/admin/storage/lifecycle/preview", rules),
};

type StorageApi = Pick<typeof storageAdminApi, "overview" | "setLifecycle" | "preview">;

/** The sampled breakdown costs the database a few seconds: never refetch on focus. */
export const useStorageOverview = (client: StorageApi = storageAdminApi) =>
  createQuery(() => ({
    queryKey: qk.adminStorage,
    queryFn: client.overview,
    staleTime: 60_000,
    refetchOnWindowFocus: false,
    retry: false,
  }));

export function storageLifecycleActions(qc: QueryClient, client: StorageApi = storageAdminApi) {
  return {
    preview: (rules: StorageLifecycleRules) => client.preview(rules),
    save: async (rules: StorageLifecycleRules) => {
      const lifecycle = await client.setLifecycle(rules);
      qc.setQueryData<StorageOverview>(qk.adminStorage, (old) => (old ? { ...old, lifecycle } : old));
      return lifecycle;
    },
    refresh: () => qc.invalidateQueries({ queryKey: qk.adminStorage }),
  };
}

export const useStorageLifecycleActions = (client: StorageApi = storageAdminApi) =>
  storageLifecycleActions(useQueryClient(), client);
