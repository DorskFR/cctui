import { createQuery } from "@tanstack/svelte-query";
import type { SessionHookPolicy } from "@bindings/SessionHookPolicy";
import { path } from "@bindings/routes";
import { api } from "../api";
import { qk } from "./keys";

export const useSessionHookPolicy = (id: () => string, enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.hookPolicy(id()),
    queryFn: () =>
      api.get<SessionHookPolicy>(path("get_sessions_by_id_hook_policy", { id: id() })),
    enabled: enabled() && !!id(),
    staleTime: 60_000,
    retry: false,
  }));
