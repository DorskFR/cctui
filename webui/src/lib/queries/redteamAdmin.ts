import { createQuery } from "@tanstack/svelte-query";
import type { RedteamOverview } from "@bindings/RedteamOverview";
import { api } from "../api";

export const redteamAdminApi = {
  overview: () => api.get<RedteamOverview>("/admin/redteam"),
};

const KEY = ["admin-redteam"] as const;

export const useRedteamOverview = (client: typeof redteamAdminApi = redteamAdminApi) =>
  createQuery(() => ({
    queryKey: KEY,
    queryFn: client.overview,
    staleTime: 60_000,
    retry: false,
  }));
