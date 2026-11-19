import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { LaunchPreset } from "@bindings/LaunchPreset";
import type { WebhookRequest } from "@bindings/WebhookRequest";
import { path } from "@bindings/routes";
import { api } from "../api";
import { enableExternalSources } from "../webhooks/externalSources";
import { endpoints } from "./endpoints";

const webhooksKey = ["webhooks"] as const;
const deliveriesKey = (id: string) => ["webhooks", "deliveries", id] as const;
const presetsKey = ["presets"] as const;

const presetIo = {
  get: (id: string) => api.get<LaunchPreset>(path("get_presets_by_id", { id })),
  put: endpoints.updatePreset,
};

export const useWebhooks = () =>
  createQuery(() => ({
    queryKey: webhooksKey,
    queryFn: endpoints.webhooks,
    refetchInterval: 30_000,
  }));

export const useWebhookDeliveries = (id: () => string) =>
  createQuery(() => ({
    queryKey: deliveriesKey(id()),
    queryFn: () => endpoints.webhookDeliveries(id()),
    refetchInterval: 15_000,
  }));

export function useWebhookActions() {
  const qc = useQueryClient();
  const settle = async <T>(work: Promise<T>): Promise<T> => {
    try {
      return await work;
    } finally {
      await qc.invalidateQueries({ queryKey: webhooksKey });
    }
  };
  return {
    create: (body: WebhookRequest) => settle(endpoints.createWebhook(body)),
    update: (id: string, body: WebhookRequest) =>
      settle(endpoints.updateWebhook(id, body)),
    remove: (id: string) => settle(endpoints.deleteWebhook(id)),
    rotate: (id: string) => settle(endpoints.rotateWebhookSecret(id)),
    acceptExternalSources: async (presetId: string) => {
      try {
        return await enableExternalSources(presetId, presetIo);
      } finally {
        await qc.invalidateQueries({ queryKey: presetsKey });
      }
    },
  };
}
