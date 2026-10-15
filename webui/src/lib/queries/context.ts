import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { ContextItem } from "@bindings/ContextItem";
import type { ContextItemSpec } from "@bindings/ContextItemSpec";
import type { AttachContextResponse } from "@bindings/AttachContextResponse";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

export const useContextItems = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.context,
    queryFn: endpoints.contextItems,
    enabled: enabled(),
  }));

export interface ContextCoords {
  machine_id?: string;
  working_dir?: string;
  /** Label ids. */
  labels?: string[];
  adapter?: string;
}

export function resolveParams(c: ContextCoords): Record<string, string> {
  const out: Record<string, string> = {};
  if (c.machine_id) out.machine_id = c.machine_id;
  if (c.working_dir?.trim()) out.working_dir = c.working_dir.trim();
  if (c.labels?.length) out.labels = [...c.labels].sort().join(",");
  if (c.adapter) out.adapter = c.adapter;
  return out;
}

export const useResolvedContext = (
  coords: () => ContextCoords,
  enabled: () => boolean = () => true,
) =>
  createQuery(() => {
    const params = resolveParams(coords());
    return {
      queryKey: qk.contextResolve(params),
      queryFn: () => endpoints.resolveContext(params),
      enabled: enabled() && !!params.machine_id,
    };
  });

export const useSessionContext = (
  sessionId: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(() => ({
    queryKey: qk.sessionContext(sessionId()),
    queryFn: () => endpoints.sessionContext(sessionId()),
    enabled: enabled() && !!sessionId(),
  }));

export function useContextActions() {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: qk.context });
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
    approvePending: async (id: string): Promise<ContextItem> => {
      const item = await endpoints.approveContextPending(id);
      await invalidate();
      return item;
    },
    rejectPending: async (id: string): Promise<ContextItem> => {
      const item = await endpoints.rejectContextPending(id);
      await invalidate();
      return item;
    },
  };
}

export function useAttachContext() {
  const qc = useQueryClient();
  return async (
    sessionId: string,
    names: string[],
  ): Promise<AttachContextResponse> => {
    const res = await endpoints.attachSessionContext(sessionId, { names });
    await qc.invalidateQueries({ queryKey: qk.sessionContext(sessionId) });
    return res;
  };
}
