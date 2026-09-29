import { getContext } from "svelte";
import type { Component } from "svelte";

export const PLUGIN_ID = "ghreview";

export const HOST_CONTEXT_KEY = "cctui:host";

export interface PageProps {
  basePath: string;
  path: string;
  navigate(path: string): void;
}

export interface HostUser {
  id: string;
  name: string;
  isAdmin: boolean;
}

export interface SpawnRequest {
  prompt: string;
  working_dir?: string;
  machine_id?: string;
}

export interface HostContext {
  cctuiApi: number;
  origin: string;
  user?: HostUser;
  apiFetch?(path: string, init?: RequestInit): Promise<Response>;
  pluginFetch?(path: string, init?: RequestInit): Promise<Response>;
  navigate?(path: string): void;
  openSpawn?(request: SpawnRequest): void;
  toast?(message: string, tone?: "ok" | "info" | "error"): void;
}

export interface CctuiPluginModule {
  cctuiApi: 1;
  page?: Component<PageProps>;
}

export function hostContext(): HostContext | undefined {
  return getContext<HostContext | undefined>(HOST_CONTEXT_KEY);
}

export function backendPath(path: string): string {
  return `/api/v1/plugins/${PLUGIN_ID}/backend${path.startsWith("/") ? path : `/${path}`}`;
}
