import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

const toolGuardKey = (id: string) => ["tool-guard", id] as const;

export const useToolGuard = (id: () => string, enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: toolGuardKey(id()),
    queryFn: () => endpoints.toolGuard(id()),
    enabled: enabled() && !!id(),
    staleTime: 30_000,
    retry: false,
  }));

export function useToolGuardActions() {
  const qc = useQueryClient();
  return {
    set: async (id: string, bypass: boolean) => {
      const state = await endpoints.putToolGuard(id, bypass);
      qc.setQueryData(toolGuardKey(id), state);
      void qc.invalidateQueries({ queryKey: qk.sessionsAll });
      return state;
    },
  };
}
