import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { ContextItem } from "@bindings/ContextItem";
import type { ContextItemSpec } from "@bindings/ContextItemSpec";
import { endpoints } from "./endpoints";

export const CONTEXT_KEY = ["context"] as const;

/** The caller's memory notes and prompt templates. */
export const useContextItems = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: CONTEXT_KEY,
    queryFn: endpoints.contextItems,
    enabled: enabled(),
  }));

/**
 * What a spawn into these coordinates would attach on its own — the set the
 * spawn modal pre-checks. Re-runs as the user changes machine or directory.
 */
export const useResolvedContext = (
  coords: () => { machine_id?: string; working_dir?: string; labels?: string },
  enabled: () => boolean = () => true,
) =>
  createQuery(() => ({
    queryKey: [...CONTEXT_KEY, "resolve", coords()],
    queryFn: () => endpoints.resolveContext(coords()),
    enabled: enabled(),
  }));

export function useContextActions() {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: CONTEXT_KEY });
  return {
    create: async (spec: ContextItemSpec): Promise<ContextItem> => {
      const item = await endpoints.createContextItem(spec);
      await invalidate();
      return item;
    },
    update: async (id: string, spec: ContextItemSpec): Promise<ContextItem> => {
      const item = await endpoints.updateContextItem(id, { spec });
      await invalidate();
      return item;
    },
    remove: async (id: string): Promise<void> => {
      await endpoints.deleteContextItem(id);
      await invalidate();
    },
  };
}
