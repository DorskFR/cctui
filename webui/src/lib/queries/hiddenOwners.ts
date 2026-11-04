import { type QueryClient, createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { HiddenOwnerKind } from "@bindings/HiddenOwnerKind";
import type { HiddenOwners } from "@bindings/HiddenOwners";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

export const useHiddenOwners = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.hiddenOwners,
    queryFn: endpoints.hiddenOwners,
    enabled: enabled(),
  }));

type HiddenOwnerApi = Pick<typeof endpoints, "hideOwner" | "unhideOwner">;

export const invalidateHiddenOwners = (qc: QueryClient) =>
  Promise.all([
    qc.invalidateQueries({ queryKey: qk.hiddenOwners }),
    qc.invalidateQueries({ queryKey: qk.sessionsAll }),
  ]);

export function hiddenOwnerActions(qc: QueryClient, api: HiddenOwnerApi = endpoints) {
  const patch = (kind: HiddenOwnerKind, targetId: string, hidden: boolean, label?: string | null) =>
    qc.setQueryData<HiddenOwners>(qk.hiddenOwners, (old) => {
      if (!old) return old;
      const rest = old.owners.filter((o) => !(o.kind === kind && o.target_id === targetId));
      return { owners: hidden ? [...rest, { kind, target_id: targetId, label: label ?? null }] : rest };
    });
  return {
    hide: async (kind: HiddenOwnerKind, targetId: string, label?: string | null) => {
      await api.hideOwner(kind, targetId);
      patch(kind, targetId, true, label);
      await invalidateHiddenOwners(qc);
    },
    unhide: async (kind: HiddenOwnerKind, targetId: string) => {
      await api.unhideOwner(kind, targetId);
      patch(kind, targetId, false);
      await invalidateHiddenOwners(qc);
    },
  };
}

export const useHiddenOwnerActions = () => hiddenOwnerActions(useQueryClient());
