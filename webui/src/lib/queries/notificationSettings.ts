import { type QueryClient, createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { CreateNotificationEndpoint } from "@bindings/CreateNotificationEndpoint";
import type { NotificationEndpoint } from "@bindings/NotificationEndpoint";
import { mergeNotifications } from "../notificationPrefs";
import { endpoints } from "./endpoints";
import { qk } from "./keys";

export const useNotificationSettings = () =>
  createQuery(() => ({
    queryKey: qk.notificationSettings,
    queryFn: async () => mergeNotifications(await endpoints.notificationSettings()),
  }));

export const useNotificationEndpoints = () =>
  createQuery(() => ({
    queryKey: qk.notificationEndpoints,
    queryFn: async (): Promise<NotificationEndpoint[]> => {
      const list: unknown = await endpoints.notificationEndpoints();
      return Array.isArray(list) ? (list as NotificationEndpoint[]) : [];
    },
  }));

type EndpointApi = Pick<
  typeof endpoints,
  "createNotificationEndpoint" | "deleteNotificationEndpoint" | "testNotificationEndpoint"
>;

export function notificationEndpointActions(qc: QueryClient, api: EndpointApi = endpoints) {
  const refresh = () => qc.invalidateQueries({ queryKey: qk.notificationEndpoints });
  return {
    create: async (body: CreateNotificationEndpoint) => {
      const created = await api.createNotificationEndpoint(body);
      await refresh();
      return created;
    },
    remove: async (id: string) => {
      await api.deleteNotificationEndpoint(id);
      await refresh();
    },
    test: (id: string) => api.testNotificationEndpoint(id),
  };
}

export const useNotificationEndpointActions = () =>
  notificationEndpointActions(useQueryClient());
