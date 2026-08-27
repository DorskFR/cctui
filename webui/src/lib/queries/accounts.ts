import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import type { PutRedirectRequest } from "@bindings/PutRedirectRequest";
import { endpoints } from "./endpoints";
import { qk } from "./keys";
import type {
  CreateAccount,
  CreateProvider,
  GrantShare,
  OAuthFinish,
  UpdateAccount,
  UpdateProvider,
} from "./types";

export const useAccounts = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: ["accounts"],
    queryFn: endpoints.accounts,
    enabled: enabled(),
  }));

export const useRedirects = (enabled: () => boolean = () => true) =>
  createQuery(() => ({
    queryKey: ["redirects"],
    queryFn: endpoints.redirects,
    enabled: enabled(),
  }));

/** Set/clear launch-time redirect rules; both invalidate the rules query. */
export function useRedirectActions() {
  const qc = useQueryClient();
  const invalidate = () => qc.invalidateQueries({ queryKey: ["redirects"] });
  return {
    put: async (accountId: string, body: PutRedirectRequest) => {
      const r = await endpoints.putRedirect(accountId, body);
      invalidate();
      return r;
    },
    remove: async (id: string) => {
      await endpoints.deleteRedirect(id);
      invalidate();
    },
  };
}

/** Per-account subscription usage. Lazy + slow-refresh: only fetched
 *  while the accounts view is mounted (caller gates `enabled`), and re-polled on
 *  a slow 3-minute interval that matches the server-side cache TTL so Anthropic's
 *  rate-limited usage endpoint is never spammed. Codex accounts return `null`. */
export const useAccountUsage = (
  accountId: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(() => ({
    queryKey: ["account-usage", accountId()],
    queryFn: () => endpoints.accountUsage(accountId()),
    enabled: enabled(),
    staleTime: 180_000,
    refetchInterval: 180_000,
    refetchOnWindowFocus: false,
    retry: false,
  }));

/** Who an account is shared with. Owner-scoped: the server 404s the
 *  list for a non-owner, so callers gate `enabled` to the account owner/admin. */
export const useAccountShares = (
  accountId: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(() => ({
    queryKey: qk.accountShares(accountId()),
    queryFn: () => endpoints.accountShares(accountId()),
    enabled: enabled(),
    retry: false,
  }));

/** Who a resource is shared with, for any shareable kind. Owner-scoped
 *  server-side (404s for a non-owner), so callers gate `enabled` accordingly. */
export const useResourceShares = (
  resourceType: () => string,
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(() => ({
    queryKey: qk.resourceShares(resourceType(), id()),
    queryFn: () => endpoints.resourceShares(resourceType(), id()),
    enabled: enabled(),
    retry: false,
  }));

/** Grant/revoke actions for generic resource sharing; each invalidates
 *  that resource's shares query. */
export function useResourceShareActions() {
  const qc = useQueryClient();
  return {
    grant: async (resourceType: string, id: string, body: GrantShare) => {
      const r = await endpoints.grantResourceShare(resourceType, id, body);
      qc.invalidateQueries({ queryKey: qk.resourceShares(resourceType, id) });
      return r;
    },
    revoke: async (resourceType: string, id: string, userId: string) => {
      await endpoints.revokeResourceShare(resourceType, id, userId);
      qc.invalidateQueries({ queryKey: qk.resourceShares(resourceType, id) });
    },
  };
}

/** CRUD for the caller's own OAuth accounts. Invalidates the accounts
 *  list after a mutation. */
export function useAccountActions() {
  const qc = useQueryClient();
  const inval = () => qc.invalidateQueries({ queryKey: ["accounts"] });
  return {
    create: async (body: CreateAccount) => {
      const r = await endpoints.createAccount(body);
      inval();
      return r;
    },
    // "Sign in with Claude": start returns the authorize URL the
    // page opens in a new tab; finish exchanges the pasted code for tokens
    // and creates the account (no inval needed on start, only on finish).
    oauthStart: (provider: string, userId?: string, accountId?: string) =>
      endpoints.oauthStart(provider, userId, accountId),
    oauthFinish: async (body: OAuthFinish) => {
      const r = await endpoints.oauthFinish(body);
      inval();
      return r;
    },
    update: async (id: string, body: UpdateAccount) => {
      const r = await endpoints.updateAccount(id, body);
      inval();
      return r;
    },
    updateProvider: async (accountId: string, providerId: string, body: UpdateProvider) => {
      const r = await endpoints.updateProvider(accountId, providerId, body);
      inval();
      return r;
    },
    addProvider: async (accountId: string, body: CreateProvider) => {
      const r = await endpoints.addProvider(accountId, body);
      inval();
      return r;
    },
    removeProvider: async (accountId: string, providerId: string) => {
      await endpoints.deleteProvider(accountId, providerId);
      inval();
    },
    moveProvider: async (accountId: string, providerId: string, targetAccountId: string) => {
      const r = await endpoints.moveProvider(accountId, providerId, targetAccountId);
      inval();
      return r;
    },
    remove: async (id: string) => {
      await endpoints.deleteAccount(id);
      inval();
    },
    // Sharing: grant/revoke invalidate that account's shares query.
    grantShare: async (id: string, body: GrantShare) => {
      const r = await endpoints.grantShare(id, body);
      qc.invalidateQueries({ queryKey: qk.accountShares(id) });
      return r;
    },
    revokeShare: async (id: string, userId: string) => {
      await endpoints.revokeShare(id, userId);
      qc.invalidateQueries({ queryKey: qk.accountShares(id) });
    },
  };
}
