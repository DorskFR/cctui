import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { api } from "../api";
import { endpoints } from "./endpoints";
import { qk } from "./keys";
import type { CreateUserResponse } from "@bindings/CreateUserResponse";
import type { MintTokenResponse } from "@bindings/MintTokenResponse";
import type { MintKeyResponse } from "@bindings/MintKeyResponse";

export const useUsers = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: qk.users,
    queryFn: endpoints.users,
    enabled: enabled(),
  }));

/** A user's scope ceiling. Self or admin. */
export const useUserAcls = (userId: () => string) =>
  createQuery(() => ({
    queryKey: qk.userAcls(userId()),
    queryFn: () => endpoints.userAcls(userId()),
    enabled: !!userId(),
  }));

/** A user's api_keys with granted scopes. Self or admin. */
export const useUserKeys = (userId: () => string) =>
  createQuery(() => ({
    queryKey: qk.userKeys(userId()),
    queryFn: () => endpoints.userKeys(userId()),
    enabled: !!userId(),
  }));

export const useAllMachines = (enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: ["machines", "all"],
    queryFn: endpoints.allMachines,
    enabled: enabled(),
  }));

export const useMachines = (userId: () => string, enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.machines(userId()),
    queryFn: () => endpoints.machines(userId()),
    enabled: enabled(),
  }));

export const useTokens = (userId: () => string, enabled: () => boolean) =>
  createQuery(() => ({
    queryKey: qk.tokens(userId()),
    queryFn: () => endpoints.tokens(userId()),
    enabled: enabled(),
  }));

export function useUserActions() {
  const qc = useQueryClient();
  const invalUsers = () => qc.invalidateQueries({ queryKey: qk.users });
  const invalUser = (userId: string) =>
    qc.invalidateQueries({ queryKey: ["users", userId] });
  return {
    create: async (name: string): Promise<CreateUserResponse> => {
      const r = await api.post<CreateUserResponse>("/admin/users", { name });
      invalUsers();
      return r;
    },
    rename: async (id: string, name: string) => {
      await api.patch<void>(`/admin/users/${id}`, { name });
      invalUsers();
    },
    setCanDispatch: async (id: string, canDispatch: boolean) => {
      await api.patch<void>(`/admin/users/${id}`, {
        can_dispatch: canDispatch,
      });
      invalUsers();
    },
    // Temporary on/off switch — unlike revoke, nothing is
    // invalidated; flipping back restores all tokens + machines.
    setDisabled: async (id: string, disabled: boolean) => {
      await api.patch<void>(`/admin/users/${id}`, { disabled });
      invalUsers();
    },
    revoke: async (id: string) => {
      await api.del<void>(`/admin/users/${id}`);
      invalUsers();
    },
    purgeUser: async (id: string) => {
      await api.del<void>(`/admin/users/${id}/purge`);
      invalUsers();
    },
    mintToken: async (
      userId: string,
      label: string | null,
    ): Promise<MintTokenResponse> => {
      const r = await api.post<MintTokenResponse>(`/users/${userId}/tokens`, {
        label,
      });
      invalUser(userId);
      return r;
    },
    relabelToken: async (
      userId: string,
      tokenId: string,
      label: string | null,
    ) => {
      await api.patch<void>(`/admin/users/${userId}/tokens/${tokenId}`, {
        label,
      });
      invalUser(userId);
    },
    revokeToken: async (userId: string, tokenId: string) => {
      await api.del<void>(`/admin/users/${userId}/tokens/${tokenId}`);
      invalUser(userId);
    },
    purgeToken: async (userId: string, tokenId: string) => {
      await api.del<void>(`/admin/users/${userId}/tokens/${tokenId}/purge`);
      invalUser(userId);
    },
    // The PATCH replaces both fields (display_name + hue), so callers pass
    // the full pair — send the current value for the field they didn't touch.
    updateMachine: async (
      userId: string,
      id: string,
      displayName: string | null,
      hue: number | null,
    ) => {
      await api.patch<void>(`/admin/machines/${id}`, {
        display_name: displayName,
        hue,
      });
      invalUser(userId);
    },
    revokeMachine: async (userId: string, id: string) => {
      await api.del<void>(`/admin/machines/${id}`);
      invalUser(userId);
    },
    purgeMachine: async (userId: string, id: string) => {
      await api.del<void>(`/admin/machines/${id}/purge`);
      invalUser(userId);
    },
    // Edits a user's ceiling (admin only); re-intersects every key at the
    // next request, and purges the server auth cache immediately.
    setUserScopes: async (userId: string, scopes: string[]) => {
      await api.patch<void>(`/users/${userId}/acls`, { scopes });
      invalUser(userId);
      invalUsers();
    },
    // Mint a scoped key (self or admin). The grant is clamped to ⊆ the owner's
    // ceiling server-side; the plaintext is returned ONCE.
    mintKey: async (
      userId: string,
      label: string | null,
      scopes: string[],
    ): Promise<MintKeyResponse> => {
      const r = await api.post<MintKeyResponse>(`/users/${userId}/keys`, {
        label,
        scopes,
        expires_at: null,
      });
      invalUser(userId);
      return r;
    },
    // Edit a key's granted scopes IN PLACE — the secret is untouched, so the
    // token keeps working. Takes effect immediately (cache purge).
    setKeyScopes: async (userId: string, keyId: string, scopes: string[]) => {
      await api.patch<void>(`/users/${userId}/keys/${keyId}/acls`, { scopes });
      invalUser(userId);
    },
    revokeKey: async (userId: string, keyId: string) => {
      await api.del<void>(`/users/${userId}/keys/${keyId}`);
      invalUser(userId);
    },
  };
}
