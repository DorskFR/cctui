import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { ContextItem } from "@bindings/ContextItem";
import type { ContextItemSpec } from "@bindings/ContextItemSpec";
import type { AttachContextResponse } from "@bindings/AttachContextResponse";
import type { ResolvedContextResponse } from "@bindings/ResolvedContextResponse";
import type { SessionContextResponse } from "@bindings/SessionContextResponse";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

const list = <T>(v: unknown): T[] => (Array.isArray(v) ? v : []);
const field = (v: unknown, key: string): unknown =>
  v && typeof v === "object" ? (v as Record<string, unknown>)[key] : undefined;

/** A server that predates these endpoints answers 404 or another shape; the
 * drawer and spawn modal must still render, so every body is coerced. */
export const asItems = (v: unknown): ContextItem[] =>
  list<ContextItem>(v)
    .filter((i) => !!i && typeof i === "object")
    .map((i) => ({ ...i, tags: list<string>(i.tags) }));
export const asResolved = (v: unknown): ResolvedContextResponse => ({
  rules: asItems(field(v, "rules")),
  memories: asItems(field(v, "memories")),
});
export const asSessionContext = (v: unknown): SessionContextResponse => ({
  rules: list(field(v, "rules")),
  memories: list(field(v, "memories")),
});
export const asAttached = (v: unknown): AttachContextResponse => ({
  attached: list(field(v, "attached")),
  unknown: list(field(v, "unknown")),
});

export const useContextItems = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.context,
    queryFn: async () => asItems(await endpoints.contextItems()),
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
      queryFn: async () => asResolved(await endpoints.resolveContext(params)),
      retry: false,
      enabled: enabled() && !!params.machine_id,
    };
  });

export const useSessionContext = (
  sessionId: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(() => ({
    queryKey: qk.sessionContext(sessionId()),
    queryFn: async () =>
      asSessionContext(await endpoints.sessionContext(sessionId())),
    retry: false,
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
    const res = asAttached(
      await endpoints.attachSessionContext(sessionId, { names }),
    );
    await qc.invalidateQueries({ queryKey: qk.sessionContext(sessionId) });
    return res;
  };
}
