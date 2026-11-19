import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { LaunchPreset } from "@bindings/LaunchPreset";
import type { LaunchPresetRequest } from "@bindings/LaunchPresetRequest";
import { presetLane } from "#lib/schedules/runs.ts";
import { endpoints } from "./endpoints";

const presetsKey = ["presets"] as const;
const runsKey = (presetId: string) => ["presets", "runs", presetId] as const;

export const usePresets = () =>
  createQuery(() => ({
    queryKey: presetsKey,
    queryFn: endpoints.presets,
    refetchInterval: 30_000,
  }));

export const usePresetRuns = (presetId: () => string, enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: runsKey(presetId()),
    queryFn: () => endpoints.inboxLaneEvents(presetLane(presetId())),
    enabled: enabled(),
    refetchInterval: 15_000,
  }));

export function usePresetActions() {
  const qc = useQueryClient();
  const refresh = () => qc.invalidateQueries({ queryKey: presetsKey });
  const settle = async <T>(work: Promise<T>): Promise<T> => {
    try {
      return await work;
    } finally {
      await refresh();
    }
  };
  return {
    save: (id: string | null, body: LaunchPresetRequest): Promise<LaunchPreset> =>
      settle(id ? endpoints.updatePreset(id, body) : endpoints.createPreset(body)),
    setFiles: (id: string, files: File[]) => settle(endpoints.setPresetFiles(id, files)),
    remove: (id: string) => settle(endpoints.deletePreset(id)),
    run: (id: string) => settle(endpoints.runPreset(id)),
  };
}
