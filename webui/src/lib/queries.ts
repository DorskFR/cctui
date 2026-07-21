import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { toStore } from "svelte/store";
import { api, ApiError } from "./api";
import type { SessionListResponse } from "@bindings/SessionListResponse";
import type { SessionStats } from "@bindings/SessionStats";
import type { TokenUsageWindows } from "@bindings/TokenUsageWindows";
import type { UsageAnalytics } from "@bindings/UsageAnalytics";
import type { SessionListItem } from "@bindings/SessionListItem";
import type { AgentEvent } from "@bindings/AgentEvent";
import type { SpawnRequest } from "@bindings/SpawnRequest";
import type { SpawnResponse } from "@bindings/SpawnResponse";
import type { ForkRequest } from "@bindings/ForkRequest";
import type { ForkResponse } from "@bindings/ForkResponse";
import type { StageFilesResponse } from "@bindings/StageFilesResponse";
import type { DispatchRequest } from "@bindings/DispatchRequest";
import type { DispatchResponse } from "@bindings/DispatchResponse";
import type { UserRow } from "@bindings/UserRow";
import type { MachineRow } from "@bindings/MachineRow";
import type { UserTokenRow } from "@bindings/UserTokenRow";
import type { CreateUserResponse } from "@bindings/CreateUserResponse";
import type { RotateResponse } from "@bindings/RotateResponse";
import type { MintTokenResponse } from "@bindings/MintTokenResponse";
import type { UserAclsResponse } from "@bindings/UserAclsResponse";
import type { ApiKeyRow } from "@bindings/ApiKeyRow";
import type { MintKeyResponse } from "@bindings/MintKeyResponse";
import type { VersionInfo } from "@bindings/VersionInfo";
import type { MeResponse } from "@bindings/MeResponse";
import type { CapabilitiesResponse } from "@bindings/CapabilitiesResponse";
import type { LangfuseSessionUsage } from "@bindings/LangfuseSessionUsage";
import type { CodexModelCatalog } from "@bindings/CodexModelCatalog";
import type { ConnectorInfo } from "@bindings/ConnectorInfo";
import type { CreateConnector } from "@bindings/CreateConnector";
import type { UpdateConnector } from "@bindings/UpdateConnector";
import type { Prompt } from "@bindings/Prompt";
import type { Label } from "@bindings/Label";
import type { LabelListResponse } from "@bindings/LabelListResponse";
import type { SettingsCatalogResponse } from "@bindings/SettingsCatalogResponse";
import type { SessionDiagnoseResponse } from "@bindings/SessionDiagnoseResponse";

/** Machine kinds the server manages itself — the per-user `dispatch` machine
 * and one-shot `ephemeral` worker pods. They are never spawn targets and are
 * hidden from the "new machines" list in the UI (CCT-183 / CCT-185). */
export const SYSTEM_MACHINE_KINDS = new Set(["dispatch", "ephemeral"]);

/** An enrolled dispatcher (CCT-285): a standalone executor service enrolled per
 *  account that dials out over `/api/v1/dispatcher/ws`. Identity record only —
 *  the enrollment key is shown once at enroll time and never echoed here. */
export interface UserDispatcher {
  id: string;
  name: string;
  /** Reported by the binary at enroll: `kubernetes` | `docker` | `http`. */
  kind: string;
  /** Non-secret fragment of the enrollment key, for display. */
  key_preview: string | null;
  /** Liveness tier derived from `last_seen_at`: `online` | `stale` | `offline`. */
  liveness: string;
  /** Whether a live WS connection is currently registered. */
  connected: boolean;
  last_seen_at: string;
  created_at: string;
  updated_at: string;
}

/** Rename payload for an enrolled dispatcher. */
export interface RenameDispatcher {
  name: string;
}

/** Response to a dispatcher enroll — `dispatcher_key` is shown ONCE. */
export interface EnrollDispatcherResponse {
  dispatcher_id: string;
  dispatcher_key: string;
  server_version: string;
}

/** One selectable model on a compatible-endpoint account (CCT-399). */
export interface AccountModel {
  model: string;
  label: string;
}

/** One provider credential under an account identity (CCT-558). Tokens are
 *  never returned by the API — only provider/expiry/last-used + lightweight
 *  usage stats. `provider` is `anthropic` (claude), `openai` (codex), or a
 *  `*-compatible` base-url-overridden endpoint (CCT-399). */
export interface AccountProvider {
  id: string;
  account_id: string;
  /** `anthropic` | `openai` (native) | `anthropic-compatible` |
   *  `openai-compatible` (a base-url-overridden endpoint, CCT-399). */
  provider: string;
  /** Provider family: `anthropic` | `openai`. At most one provider per family
   *  per account (CCT-508 guard by construction). */
  family: string;
  /** Selectable models for a compatible endpoint (CCT-399); null/empty for
   *  native subscription accounts (which use the harness's native families).
   *  Safe to surface — model names aren't secret (unlike base URL + credential). */
  models: AccountModel[] | null;
  /** Per-provider logical→concrete model alias map (CCT-406), e.g.
   *  `{ opus: "claude-opus-4-8[1m]" }`. Resolved server-side at spawn.
   *  null/empty means no remapping. */
  model_aliases: Record<string, string> | null;
  /** True for a server-synthesized provider (the CCTUI_CLAUDE_LITELLM_* shim) —
   *  read-only: edit/delete are rejected server-side (CCT-399). */
  managed: boolean;
  /** Compatible-endpoint base URL (CCT-399); null for native providers. */
  base_url: string | null;
  /** `oauth` (native) | `bearer` | `api_key` (compatible, CCT-399). */
  auth_scheme: string;
  provider_account_id: string | null;
  expires_at: string | null;
  created_at: string;
  last_used_at: string | null;
  request_count: number;
  bytes_transferred: number;
  /** Total tokens (input + output + cache) across this provider's sessions. */
  total_tokens: number;
  /** Rough USD cost estimate from tokens (per-provider blended rate, CCT-273). */
  est_cost_usd: number;
  /** Per-provider soft limits on cctui's own share of the usage windows
   *  (CCT-411/CCT-688), keyed by canonical window id (`session` | `weekly_all` |
   *  `weekly_model:<slug>`). Absent key ⇒ no config for that window; null map ⇒
   *  none at all. The per-window bypass (CCT-484) ignores that window's cap when
   *  it resets within that many minutes. */
  soft_limits: Record<string, SoftLimitConfig> | null;
  /** Credential health (CCT-512): true once the gateway saw the upstream provider
   *  reject this credential; cleared on the next successful upstream call. UI
   *  shows a "reauthenticate" badge + button when set. */
  needs_reauth: boolean;
  last_auth_error: string | null;
  last_auth_error_at: string | null;
  /** Validated, allowlisted harness settings applied to sessions run under this
   *  provider (CCT-538/CCT-541). Config, not secret → returned. Only SAFE/CARE
   *  settings.json keys; the server rejects MANAGED/SYSTEM keys on write. */
  settings_json: Record<string, unknown> | null;
}

/** An account identity (CCT-558): name + owner, with zero or more provider
 *  credentials attached. The pre-CCT-558 flat shape (one row = one credential)
 *  became `providers[0]` for existing data. */
export interface OAuthAccount {
  id: string;
  name: string;
  /** Owning user (CCT-251) — shown to admins, who see all accounts. */
  user_id: string;
  user_name: string | null;
  created_at: string;
  updated_at: string;
  providers: AccountProvider[];
  /** Names (only) of the account's free-form extra env vars (CCT-591), sorted.
   *  Values stay write-only (never returned); the names drive the "currently
   *  set" display + replace-on-save affordance in the account editor. */
  env_names: string[];
  // NOTE: env VALUES are deliberately absent — the `env_json` blob is write-only,
  // never returned over the API (may hold secrets), exactly like the OAuth tokens.
}

/** Single-provider back-compat: the spawn/dispatch pickers derive the
 *  credential from the account's provider-family union (CCT-562); the
 *  remaining first-row reader is DispatchersPanel until it grows a provider
 *  dimension. */
export const primaryProvider = (a: OAuthAccount): AccountProvider | undefined => a.providers[0];

/** One of a session's active per-family gateway credential bindings. */
export interface SessionBinding {
  family: "anthropic" | "openai";
  credential_id: string;
  account_id: string;
  account_name: string;
}

/** One window's soft-limit config (CCT-688). Absent field ⇒ unset for that
 *  window: no `cap_pct` ⇒ no cap; no `bypass_minutes` ⇒ no bypass. */
export interface SoftLimitConfig {
  cap_pct?: number | null;
  bypass_minutes?: number | null;
}

/** One normalized, provider-agnostic usage window (CCT-688). `key` is the
 *  canonical id (`session` | `weekly_all` | `weekly_model:<slug>`), `label` the
 *  server-supplied display string, `utilization` a 0–100 percent (may exceed). */
export interface UsageWindow {
  key: string;
  kind: string;
  label: string;
  utilization: number;
  resets_at?: string | null;
  model_id?: string | null;
  model_display_name?: string | null;
}

/** Per-account subscription usage (CCT-306/CCT-688). `windows` is the normalized
 *  collection the UI renders (may be empty — distinct from a fetch error and from
 *  a provider with no usage API). `usage` keeps the raw upstream payload for the
 *  legacy chip. `age_secs` reflects the slow-refresh cache. */
export interface AccountUsage {
  account_id: string;
  provider: string;
  usage: {
    five_hour?: { utilization?: number | null; resets_at?: string | null } | null;
    seven_day?: { utilization?: number | null; resets_at?: string | null } | null;
    seven_day_opus?: { utilization?: number | null; resets_at?: string | null } | null;
    seven_day_sonnet?: { utilization?: number | null; resets_at?: string | null } | null;
  } | null;
  windows: UsageWindow[];
  age_secs: number;
}

/** Register payload — the refresh token is sent cleartext once, stored
 *  encrypted, and never read back. */
export interface CreateAccount {
  name: string;
  provider: string;
  /** OAuth refresh token (native subscription accounts). */
  refresh_token?: string;
  /** Initial access token (native) OR the static credential for a compatible
   *  endpoint's bearer/api key (CCT-399). */
  access_token?: string;
  expires_at?: number;
  /** Compatible-endpoint base URL (CCT-399); required for `*-compatible`. */
  base_url?: string;
  /** Selectable models for a compatible endpoint (CCT-399). */
  models?: AccountModel[];
  /** Logical→concrete model alias map (CCT-406); honoured for every provider. */
  model_aliases?: Record<string, string>;
  /** `bearer` | `api_key` for a compatible endpoint (CCT-399). */
  auth_scheme?: string;
  /** Owner — required when authenticated with the admin token (CCT-251). */
  user_id?: string;
  /** Per-account soft limits keyed by canonical window id (CCT-688). */
  soft_limits?: Record<string, SoftLimitConfig>;
}

/** Provider create/attach payload (CCT-558): `POST /accounts/{id}/providers`.
 *  The pasted-token / compatible-endpoint path — the native OAuth flows attach
 *  via `oauth/start`'s `account_id` instead. 409 when the account already has
 *  a provider of the same family (anthropic/openai). */
export interface CreateProvider {
  provider: string;
  /** OAuth refresh token (native subscription providers). */
  refresh_token?: string;
  /** Initial access token (native) OR the static credential for a compatible
   *  endpoint (CCT-399). */
  access_token?: string;
  expires_at?: number;
  /** Compatible-endpoint base URL (CCT-399); required for `*-compatible`. */
  base_url?: string;
  models?: AccountModel[];
  model_aliases?: Record<string, string>;
  /** `bearer` | `api_key` for a compatible endpoint (CCT-399). */
  auth_scheme?: string;
  soft_limits?: Record<string, SoftLimitConfig>;
  settings_json?: Record<string, unknown>;
}

/** Identity-level edit payload (CCT-558): rename and/or replace the write-only
 *  extra-env map. Provider-credential fields moved to [`UpdateProvider`]. */
export interface UpdateAccount {
  name?: string;
  /** Replacement extra-env map (CCT-538). Provided → re-encrypts and replaces
   *  (an empty map clears it); absent → unchanged. WRITE-ONLY: never returned,
   *  so the editor only ever sends new values, it can't display stored ones. */
  env_json?: Record<string, string>;
  /** Stored env var names to delete server-side (CCT-591); ignored when
   *  `env_json` is provided (replace-all wins). */
  env_remove?: string[];
}

/** Provider-credential edit payload (CCT-558, formerly the provider half of the
 *  account PATCH, CCT-402). Every field optional; an absent field leaves that
 *  column unchanged. The compatible-endpoint fields are only honoured for a
 *  non-managed `*-compatible` provider. A blank `base_url`/`access_token`
 *  keeps the stored value (they are never read back). */
export interface UpdateProvider {
  base_url?: string;
  auth_scheme?: string;
  models?: AccountModel[];
  /** Replacement alias map (CCT-406); provided replaces wholesale (empty clears),
   *  absent leaves it unchanged. Editable for every provider. */
  model_aliases?: Record<string, string>;
  /** New static credential; omit/blank to keep the stored one. */
  access_token?: string;
  /** Replacement soft-limit map (CCT-688), keyed by canonical window id.
   *  Provided → REPLACES the whole stored map ({} clears all, a dropped key
   *  removes that window's config); absent → unchanged. */
  soft_limits?: Record<string, SoftLimitConfig>;
  /** Replacement validated settings blob (CCT-538/CCT-541). Provided → replaces
   *  the stored settings wholesale (an empty object clears it); absent →
   *  unchanged. Validated against the SAFE/CARE allowlist before persist. */
  settings_json?: Record<string, unknown>;
}

/** "Sign in with Claude" OAuth start payload/response (CCT-243). */
export interface OAuthStartResponse {
  nonce: string;
  authorize_url: string;
}

/**
 * Finish payload. For Claude: the `code#state` pair pasted from claude.ai. For
 * Codex: the full localhost:1455 callback URL pasted from the address bar
 * (CCT-244).
 */
export interface OAuthFinish {
  nonce: string;
  /** New-account name; ignored when the flow was started with an attach
   *  target (`account_id` on start, CCT-558). */
  name?: string;
  code?: string;
  callback_url?: string;
}

/** One live share grant on an account (CCT-510): who it's shared with + since
 *  when. No secrets; `user_name` is the grantee's login joined for display. */
export interface ShareInfo {
  account_id: string;
  user_id: string;
  user_name: string;
  action: string;
  granted_at: string;
}

/** Grant payload (CCT-510). `user` is a UUID or a login; `action` defaults to
 *  `use` server-side. */
export interface GrantShare {
  user: string;
  action?: string;
}

/** One live share grant on any shareable resource (CCT-531): the polymorphic
 *  generalization of {@link ShareInfo}. `resource_type` is the DB kind
 *  (`account` | `machine` | `dispatcher` | `context_pack`). */
export interface ResourceShareInfo {
  resource_type: string;
  resource_id: string;
  user_id: string;
  user_name: string;
  action: string;
  granted_at: string;
}

/** Centralised query keys so invalidation stays consistent. */
export const qk = {
  version: ["version"] as const,
  capabilities: ["capabilities"] as const,
  sessions: (archived: boolean) => ["sessions", { archived }] as const,
  sessionStats: ["session-stats"] as const,
  tokenStats: ["token-stats"] as const,
  usageAnalytics: (days: number) => ["usage-analytics", { days }] as const,
  // NOT under ['sessions'] on purpose: list invalidations (`['sessions']`,
  // bumped ~every 2s while streaming) must NOT refetch the conversation —
  // a refetched history that overlaps the live ws events produced duplicate
  // messages. Live updates come through the ws listener, not refetch.
  conversation: (id: string) => ["conversation", id] as const,
  users: ["users"] as const,
  machines: (userId: string) => ["users", userId, "machines"] as const,
  tokens: (userId: string) => ["users", userId, "tokens"] as const,
  // CCT-410: per-user ceiling + per-key grants.
  userAcls: (userId: string) => ["users", userId, "acls"] as const,
  userKeys: (userId: string) => ["users", userId, "keys"] as const,
  labels: ["labels"] as const,
  accountShares: (accountId: string) => ["accounts", accountId, "shares"] as const,
  resourceShares: (resourceType: string, id: string) =>
    ["resource-shares", resourceType, id] as const,
  settingsCatalog: ["settings-catalog"] as const,
};

/** Raw typed fetchers — also usable outside of components. */
export const endpoints = {
  version: () => api.get<VersionInfo>("/version"),
  /** Which optional integrations this server has, and whether each is live
   * (CCT-375). Drives capability-gated UI: the lazy `/github` route + nav. */
  capabilities: () => api.get<CapabilitiesResponse>("/capabilities"),
  /** Who the stored bearer token resolves to (CCT-251). */
  me: () => api.get<MeResponse>("/me"),
  sessions: (archived: boolean) =>
    api.get<SessionListResponse>("/sessions", {
      include_archived: archived || undefined,
    }),
  /** Aggregate session counts for the Overview — correct past the list's
   * 25-row display cap (the list-derived counts are not). */
  sessionStats: () => api.get<SessionStats>("/sessions/stats"),
  /** Every label known to the server (CCT-360) — feeds the picker + filter. */
  labels: () => api.get<LabelListResponse>("/labels"),
  /** Token totals across rolling windows for the Overview. `tzOffset` is
   * `Date.getTimezoneOffset()` — only used to anchor "today" to local midnight. */
  tokenStats: (tzOffset: number) =>
    api.get<TokenUsageWindows>("/sessions/stats/tokens", {
      tz_offset: tzOffset,
    }),
  /** Overview usage analytics (CCT-707): tokens-over-time buckets, per-model
   * breakdown, and an hour-of-week activity heatmap. `days` sets the range +
   * bucket granularity; `tzOffset` anchors buckets/heatmap to local time. */
  usageAnalytics: (days: number, tzOffset: number) =>
    api.get<UsageAnalytics>("/sessions/stats/usage", {
      days,
      tz_offset: tzOffset,
    }),
  // Full-transcript substring search (CCT-184). `includeArchived` sets scope
  // (live-only vs all); an empty `q` with `includeArchived` browses the
  // archive. Offset-paginated.
  searchSessions: (
    q: string,
    includeArchived: boolean,
    limit: number,
    offset: number,
  ) =>
    api.get<SessionListResponse>("/sessions/search", {
      q: q || undefined,
      include_archived: includeArchived || undefined,
      limit,
      offset,
    }),
  searchFieldValues: (field: string, q: string) =>
    api.get<string[]>("/sessions/search/values", {
      field,
      q: q || undefined,
    }),
  session: (id: string) => api.get<SessionListItem>(`/sessions/${id}`),
  /** Mark this session's messages seen for the caller (CCT-580) — clears its
   *  unread badge on the next `/sessions` refetch. */
  markSeen: (id: string) => api.post<void>(`/sessions/${id}/seen`),
  conversation: (id: string) =>
    api.get<AgentEvent[]>(`/sessions/${id}/conversation`),
  /** One-call session diagnose (CCT-547): everything the daemon knows about
   *  the session — each fact dated + sourced, plus the arbitration verdict —
   *  merged with the server-side gateway/account binding facts. */
  sessionDiagnose: (id: string) =>
    api.get<SessionDiagnoseResponse>(`/sessions/${id}/diagnose`),
  /** Per-session Langfuse cost/usage rollup (CCT-564), proxied server-side so
   *  the project keys never reach the browser. */
  sessionLangfuse: (id: string) =>
    api.get<LangfuseSessionUsage>(`/sessions/${id}/langfuse`),
  recentDirs: (machineId: string) =>
    api.get<string[]>("/sessions/recent-dirs", {
      machine_id: machineId || undefined,
    }),
  /** Sub-directories of `path` on a machine, for the working-dir
   *  autocomplete in the spawn dialog. */
  machineDirs: (machineId: string, path: string) =>
    api.get<{ dirs: string[] }>(`/machines/${machineId}/fs/dirs`, { path }),
  /** Machine/account-scoped codex model catalog (CCT-641). Empty `models`
   *  when none is cached yet — the picker falls back to its static list. */
  codexModels: (machineId: string) =>
    api.get<CodexModelCatalog>(`/machines/${machineId}/codex-models`),
  users: () => api.get<UserRow[]>("/admin/users"),
  machines: (userId: string) =>
    api.get<MachineRow[]>(`/admin/users/${userId}/machines`),
  tokens: (userId: string) =>
    api.get<UserTokenRow[]>(`/admin/users/${userId}/tokens`),
  /** A user's scope ceiling (CCT-410). Self or admin. */
  userAcls: (userId: string) =>
    api.get<UserAclsResponse>(`/users/${userId}/acls`),
  /** A user's api_keys with their granted scopes (CCT-410). Self or admin. */
  userKeys: (userId: string) => api.get<ApiKeyRow[]>(`/users/${userId}/keys`),
  /** Spawn on a machine. Always `multipart/form-data` (CCT-203): the JSON
   *  `SpawnRequest` rides in the `request` part and any attached files ride as
   *  file parts the daemon stages under /tmp/cctui-uploads. */
  spawn: (body: SpawnRequest, files: File[] = []) => {
    const form = new FormData();
    form.append("request", JSON.stringify(body));
    for (const f of files) form.append("files", f, f.name);
    return api.postForm<SpawnResponse>("/sessions/spawn", form);
  },
  /** Stage mid-chat attachments for a running session (CCT-236). Same
   *  multipart shape + caps as spawn; resolves to the staged absolute paths
   *  on the session's machine, which the composer appends under the reply. */
  stageFiles: (sessionId: string, files: File[]) => {
    const form = new FormData();
    for (const f of files) form.append("files", f, f.name);
    return api.postForm<StageFilesResponse>(
      `/sessions/${sessionId}/files`,
      form,
    );
  },
  /** Fork a conversation into a new session, optionally changing model/effort
   *  (CCT-302). Returns a `command_id` to await on the ws like spawn. */
  fork: (sessionId: string, body: ForkRequest) =>
    api.post<ForkResponse>(`/sessions/${sessionId}/fork`, body),
  resume: (sessionId: string) =>
    api.post<void>(`/sessions/${sessionId}/resume`, {}),
  /** Rebind one of a session's per-family gateway bindings.
   *  Pure server-side rebind: the worker keeps running and its next upstream
   *  call in the target credential's family lands on `account` (a name or id);
   *  the other family's binding is untouched. 409 when the session carries no
   *  binding in that family. */
  switchAccount: (sessionId: string, account: string) =>
    api.post<void>(`/sessions/${sessionId}/switch-account`, { account }),
  sessionBindings: (sessionId: string) =>
    api.get<SessionBinding[]>(`/sessions/${sessionId}/bindings`),
  /** Launch a draft session (CCT-394): env is entered fresh here (never stored
   *  in the draft), account gateway tokens minted server-side at dispatch. The
   *  draft row is removed and a live session is born from the daemon. */
  launchDraft: (sessionId: string, env: Record<string, string> = {}) =>
    api.post<SpawnResponse>(`/sessions/${sessionId}/launch`, { env }),
  /** Discard (delete) a draft session row (CCT-394). */
  discardDraft: (sessionId: string) =>
    api.post<void>(`/sessions/${sessionId}/discard`, {}),
  dispatch: (body: DispatchRequest) =>
    api.post<DispatchResponse>("/sessions/dispatch", body),
  /** Configured dispatcher ids (e.g. `["claude-worker"]`); empty when none. */
  dispatchers: () => api.get<string[]>("/sessions/dispatchers"),
  /** The caller's enrolled dispatchers (CCT-285) with liveness. */
  userDispatchers: () => api.get<UserDispatcher[]>("/dispatchers"),
  /** Enroll a dispatcher; the key is returned ONCE and never echoed again. */
  enrollDispatcher: (body: { name: string; kind?: string; account?: string; provider?: string }) =>
    api.post<EnrollDispatcherResponse>("/dispatcher/enroll", body),
  updateDispatcher: (id: string, body: RenameDispatcher) =>
    api.patch<UserDispatcher>(`/dispatchers/${id}`, body),
  deleteDispatcher: (id: string) => api.del<void>(`/dispatchers/${id}`),
  /** The caller's own OAuth accounts (CCT-232). Tokens never returned. */
  accounts: () => api.get<OAuthAccount[]>("/accounts"),
  /** The per-account settings catalog (CCT-571): exposable settings keys, the
   *  curated env allowlist, and the quiet-defaults preset — served from the
   *  server's embedded catalog so the editor can never drift from what the
   *  server validates on write. */
  settingsCatalog: () =>
    api.get<SettingsCatalogResponse>("/accounts/settings-catalog"),
  createAccount: (body: CreateAccount) =>
    api.post<OAuthAccount>("/accounts", body),
  updateAccount: (id: string, body: UpdateAccount) =>
    api.patch<OAuthAccount>(`/accounts/${id}`, body),
  /** Edit one provider credential under an account (CCT-558). */
  updateProvider: (accountId: string, providerId: string, body: UpdateProvider) =>
    api.patch<AccountProvider>(`/accounts/${accountId}/providers/${providerId}`, body),
  /** Attach a provider credential to an existing account (CCT-558): the
   *  pasted-token / compatible-endpoint path. 409 on a family collision. */
  addProvider: (accountId: string, body: CreateProvider) =>
    api.post<AccountProvider>(`/accounts/${accountId}/providers`, body),
  /** Remove one provider credential; the identity + other providers stay. */
  deleteProvider: (accountId: string, providerId: string) =>
    api.del<void>(`/accounts/${accountId}/providers/${providerId}`),
  /** Re-parent a provider onto another account of the same owner (CCT-558's
   *  manual merge for the migration's one-account-per-old-row backfill). */
  moveProvider: (accountId: string, providerId: string, targetAccountId: string) =>
    api.post<AccountProvider>(`/accounts/${accountId}/providers/${providerId}/move`, {
      target_account_id: targetAccountId,
    }),
  deleteAccount: (id: string) => api.del<void>(`/accounts/${id}`),
  /** Current subscription usage for an account (CCT-306). Free + tokenless;
   *  the server slow-refreshes a cache so polling never spams upstream. */
  accountUsage: (id: string) => api.get<AccountUsage>(`/accounts/${id}/usage`),
  /** Who an account is shared with (CCT-510). Owner-scoped server-side. */
  accountShares: (id: string) => api.get<ShareInfo[]>(`/accounts/${id}/shares`),
  grantShare: (id: string, body: GrantShare) =>
    api.post<ShareInfo>(`/accounts/${id}/shares`, body),
  revokeShare: (id: string, userId: string) =>
    api.del<void>(`/accounts/${id}/shares/${userId}`),
  /** Generic resource sharing (CCT-531). `resourceType` is the DB kind
   *  (`account` | `machine` | `dispatcher` | `context_pack`); owner-scoped. */
  resourceShares: (resourceType: string, id: string) =>
    api.get<ResourceShareInfo[]>(`/${resourceType}/${id}/shares`),
  grantResourceShare: (resourceType: string, id: string, body: GrantShare) =>
    api.post<ResourceShareInfo>(`/${resourceType}/${id}/shares`, body),
  revokeResourceShare: (resourceType: string, id: string, userId: string) =>
    api.del<void>(`/${resourceType}/${id}/shares/${userId}`),
  oauthStart: (provider: string, userId?: string, accountId?: string) =>
    api.post<OAuthStartResponse>("/accounts/oauth/start", {
      provider,
      user_id: userId,
      // Attach target (CCT-558): finish lands the credential as a provider
      // under this existing account instead of creating a new identity.
      account_id: accountId,
    }),
  oauthFinish: (body: OAuthFinish) =>
    api.post<OAuthAccount>("/accounts/oauth/finish", body),
  /** GitHub connectors (GH-CONN-1). The credential is encrypted at rest and
   *  never returned — list/create only ever surface a masked preview. */
  githubConnectors: () => api.get<ConnectorInfo[]>("/github/connectors"),
  createGithubConnector: (body: CreateConnector) =>
    api.post<ConnectorInfo>("/github/connectors", body),
  updateGithubConnector: (id: string, body: UpdateConnector) =>
    api.patch<ConnectorInfo>(`/github/connectors/${id}`, body),
  deleteGithubConnector: (id: string) =>
    api.del<void>(`/github/connectors/${id}`),
  /** Run the reconcile poll for one connector immediately (CCT-396), instead of
   *  waiting for the scheduled tick. Returns the updated connector view, whose
   *  `last_polled_at`/`last_error` reflect this attempt. */
  syncGithubConnector: (id: string) =>
    api.post<ConnectorInfo>(`/github/connectors/${id}/sync`, {}),
  /** Resolve the effective repo-scoped prompt of `kind` (default `review`) for
   *  `owner/repo`, richelieu-style most-specific-wins (CCT-390): a prompt scoped
   *  to `owner/repo` beats one scoped to the whole owner, which beats a global
   *  one. Returns `null` when no candidate matches (404) so the caller can seed
   *  an empty prompt rather than treating it as an error. */
  resolveReviewPrompt: async (
    owner: string,
    repo: string,
    kind = "review",
  ): Promise<Prompt | null> => {
    try {
      return await api.get<Prompt>("/prompts/resolve", { owner, repo, kind });
    } catch (e) {
      if (e instanceof ApiError && e.status === 404) return null;
      throw e;
    }
  },
  /** Every spawnable machine across all active users — for the spawn picker.
   * Excludes server-managed machines (`ephemeral` worker pods and the per-user
   * `dispatch` machine): those aren't somewhere you'd start an interactive
   * session, only real enrolled daemons are (CCT-183 / CCT-185). */
  allMachines: async (): Promise<MachineRow[]> => {
    const users = (await api.get<UserRow[]>("/admin/users")).filter(
      (u) => !u.revoked_at,
    );
    const lists = await Promise.all(
      users.map((u) => api.get<MachineRow[]>(`/admin/users/${u.id}/machines`)),
    );
    return lists
      .flat()
      .filter((m) => !m.revoked_at && !SYSTEM_MACHINE_KINDS.has(m.kind));
  },
};

/* ---------------- Queries ----------------
 * This svelte-query build types options as `T | Readable<T>` (not an accessor
 * function), so reactive params are bridged from runes via Svelte 5's
 * `toStore(getter)`; param-less queries pass a plain options object. */

export const useMe = () =>
  createQuery({
    queryKey: ["me"],
    queryFn: endpoints.me,
    staleTime: 5 * 60_000,
  });

/** Server capability flags (CCT-375). Long stale time — capabilities only
 * change on install/uninstall, which is rare and owner-driven. */
export const useCapabilities = () =>
  createQuery({
    queryKey: qk.capabilities,
    queryFn: endpoints.capabilities,
    staleTime: 5 * 60_000,
  });

/** The settings catalog (CCT-571). Embedded server data — effectively
 * immutable per server version, so cache it for the whole session. */
export const useSettingsCatalog = () =>
  createQuery({
    queryKey: qk.settingsCatalog,
    queryFn: endpoints.settingsCatalog,
    staleTime: Infinity,
  });

export const useVersion = () =>
  createQuery({
    queryKey: qk.version,
    queryFn: endpoints.version,
    staleTime: 60_000,
  });

export const useSessions = (
  archived: () => boolean,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.sessions(archived()),
      queryFn: () => endpoints.sessions(archived()),
      refetchInterval: 15_000,
      enabled: enabled(),
    })),
  );

export const useSessionStats = () =>
  createQuery({
    queryKey: qk.sessionStats,
    queryFn: endpoints.sessionStats,
    refetchInterval: 15_000,
  });

/** All label definitions (CCT-360). Shared by the per-session picker and the
 * sessions-page filter; refetched lazily since labels change rarely. */
export const useLabels = () =>
  createQuery({
    queryKey: qk.labels,
    queryFn: endpoints.labels,
    refetchInterval: 60_000,
  });

export const useTokenStats = () =>
  createQuery({
    queryKey: qk.tokenStats,
    // Resolve the offset per fetch so it stays correct across a DST change.
    queryFn: () => endpoints.tokenStats(new Date().getTimezoneOffset()),
    refetchInterval: 15_000,
  });

export const useUsageAnalytics = (days: () => number) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.usageAnalytics(days()),
      queryFn: () =>
        endpoints.usageAnalytics(days(), new Date().getTimezoneOffset()),
      refetchInterval: 60_000,
    })),
  );

export const useConversation = (
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.conversation(id()),
      queryFn: () => endpoints.conversation(id()),
      enabled: enabled() && !!id(),
    })),
  );

/** Session diagnose panel (CCT-547). Fetched only while the panel is open;
 *  no polling — the panel offers an explicit refresh instead, since the call
 *  round-trips through the daemon. */
export const useSessionDiagnose = (
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: ["session-diagnose", id()],
      queryFn: () => endpoints.sessionDiagnose(id()),
      enabled: enabled() && !!id(),
      staleTime: 0,
      retry: false,
    })),
  );

/** Per-session Langfuse cost/usage chip (CCT-564). Lazy — fetched only while
 *  the drawer is open and the capability is present; the server caches ~60s so
 *  a short client stale time won't hammer upstream. Fail-open: on error the
 *  chip simply hides. */
export const useSessionLangfuse = (
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: ["session-langfuse", id()],
      queryFn: () => endpoints.sessionLangfuse(id()),
      enabled: enabled() && !!id(),
      staleTime: 60_000,
      retry: false,
    })),
  );

export const useRecentDirs = (machineId: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: ["recent-dirs", machineId()],
      queryFn: () => endpoints.recentDirs(machineId()),
      enabled: !!machineId(),
      staleTime: 30_000,
    })),
  );

export const useMachineDirs = (machineId: () => string, path: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: ["machine-dirs", machineId(), path()],
      queryFn: () => endpoints.machineDirs(machineId(), path()),
      enabled: !!machineId() && !!path(),
      staleTime: 10_000,
      retry: false,
    })),
  );

export const useCodexModels = (machineId: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: ["codex-models", machineId()],
      queryFn: () => endpoints.codexModels(machineId()),
      enabled: !!machineId(),
      staleTime: 60_000,
      retry: false,
    })),
  );

export const useUsers = (enabled: () => boolean = () => true) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.users,
      queryFn: endpoints.users,
      enabled: enabled(),
    })),
  );

/** A user's scope ceiling (CCT-410). Self or admin. */
export const useUserAcls = (userId: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.userAcls(userId()),
      queryFn: () => endpoints.userAcls(userId()),
      enabled: !!userId(),
    })),
  );

/** A user's api_keys with granted scopes (CCT-410). Self or admin. */
export const useUserKeys = (userId: () => string) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.userKeys(userId()),
      queryFn: () => endpoints.userKeys(userId()),
      enabled: !!userId(),
    })),
  );

export const useDispatchers = (enabled: () => boolean) =>
  createQuery(
    toStore(() => ({
      queryKey: ["dispatchers"],
      queryFn: endpoints.dispatchers,
      enabled: enabled(),
      staleTime: 60_000,
    })),
  );

export const useUserDispatchers = () =>
  createQuery({
    queryKey: ["user-dispatchers"],
    queryFn: endpoints.userDispatchers,
  });

export const useAccounts = (enabled: () => boolean = () => true) =>
  createQuery(
    toStore(() => ({
      queryKey: ["accounts"],
      queryFn: endpoints.accounts,
      enabled: enabled(),
    })),
  );

export const useSessionBindings = (sessionId: () => string, enabled: () => boolean = () => true) =>
  createQuery(
    toStore(() => ({
      queryKey: ["session-bindings", sessionId()],
      queryFn: () => endpoints.sessionBindings(sessionId()),
      enabled: enabled(),
    })),
  );

/** Per-account subscription usage (CCT-306). Lazy + slow-refresh: only fetched
 *  while the accounts view is mounted (caller gates `enabled`), and re-polled on
 *  a slow 3-minute interval that matches the server-side cache TTL so Anthropic's
 *  rate-limited usage endpoint is never spammed. Codex accounts return `null`. */
export const useAccountUsage = (
  accountId: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: ["account-usage", accountId()],
      queryFn: () => endpoints.accountUsage(accountId()),
      enabled: enabled(),
      staleTime: 180_000,
      refetchInterval: 180_000,
      refetchOnWindowFocus: false,
      retry: false,
    })),
  );

/** Who an account is shared with (CCT-510). Owner-scoped: the server 404s the
 *  list for a non-owner, so callers gate `enabled` to the account owner/admin. */
export const useAccountShares = (
  accountId: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.accountShares(accountId()),
      queryFn: () => endpoints.accountShares(accountId()),
      enabled: enabled(),
      retry: false,
    })),
  );

/** Who a resource is shared with (CCT-531), for any shareable kind. Owner-scoped
 *  server-side (404s for a non-owner), so callers gate `enabled` accordingly. */
export const useResourceShares = (
  resourceType: () => string,
  id: () => string,
  enabled: () => boolean = () => true,
) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.resourceShares(resourceType(), id()),
      queryFn: () => endpoints.resourceShares(resourceType(), id()),
      enabled: enabled(),
      retry: false,
    })),
  );

/** Grant/revoke actions for generic resource sharing (CCT-531); each invalidates
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

export type { ConnectorInfo, CreateConnector, UpdateConnector };

/** GitHub connectors (GH-CONN-1). Only fetched while the GitHub view is mounted
 *  (caller gates `enabled` on the capability). Credentials are never returned. */
export const useGithubConnectors = (enabled: () => boolean = () => true) =>
  createQuery(
    toStore(() => ({
      queryKey: ["github-connectors"],
      queryFn: endpoints.githubConnectors,
      enabled: enabled(),
    })),
  );

export function useGithubConnectorActions() {
  const qc = useQueryClient();
  // Connector changes flip the capability (repos/enabled), so refresh both.
  const inval = () => {
    qc.invalidateQueries({ queryKey: ["github-connectors"] });
    qc.invalidateQueries({ queryKey: qk.capabilities });
  };
  return {
    create: async (body: CreateConnector) => {
      const r = await endpoints.createGithubConnector(body);
      inval();
      return r;
    },
    update: async (id: string, body: UpdateConnector) => {
      const r = await endpoints.updateGithubConnector(id, body);
      inval();
      return r;
    },
    remove: async (id: string) => {
      await endpoints.deleteGithubConnector(id);
      inval();
    },
    sync: async (id: string) => {
      const r = await endpoints.syncGithubConnector(id);
      inval();
      return r;
    },
  };
}

export const useAllMachines = (enabled: () => boolean) =>
  createQuery(
    toStore(() => ({
      queryKey: ["machines", "all"],
      queryFn: endpoints.allMachines,
      enabled: enabled(),
    })),
  );

export const useMachines = (userId: () => string, enabled: () => boolean) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.machines(userId()),
      queryFn: () => endpoints.machines(userId()),
      enabled: enabled(),
    })),
  );

export const useTokens = (userId: () => string, enabled: () => boolean) =>
  createQuery(
    toStore(() => ({
      queryKey: qk.tokens(userId()),
      queryFn: () => endpoints.tokens(userId()),
      enabled: enabled(),
    })),
  );

/* ---------------- Actions (plain async + invalidation) ----------------
 * These are intentionally NOT createMutation: they return promises so callers
 * can await + toast, and they invalidate the relevant queries directly. Must
 * be called during component init (they read the query-client context). */

/** Build a placeholder card for an in-flight dispatch (CCT-193). Mirrors the
 * fields the worker will report once its daemon registers, so the optimistic
 * card looks like the real one until the refetch reconciles it by id. */
function optimisticDispatchCard(
  id: string,
  body: DispatchRequest,
): SessionListItem {
  const p = (body.payload ?? {}) as Record<string, string>;
  return {
    id,
    parent_id: null,
    machine_id: "dispatch",
    // Real cwd is unknown until the worker registers; show the target repo if
    // the payload carries one, else nothing (no `dispatch:<origin>` noise).
    working_dir: p.repo ?? "",
    status: "new",
    liveness: "stale",
    attention: null,
    bucket: "working",
    uptime_secs: 0,
    token_usage: {
      tokens_in: 0,
      tokens_out: 0,
      cost_usd: 0,
      cache_read_tokens: 0,
      cache_creation_tokens: 0,
    },
    metadata: null,
    adapter_id: "claude-code",
    machine_name: "dispatch",
    machine_hue: null,
    // Lands the optimistic card straight in the Dispatched group (CCT-231).
    machine_kind: "dispatch",
    last_message_text: "Dispatching…",
    last_message_at: null,
    registered_at: null,
    name:
      p.name ||
      p.prompt_file ||
      (p.prompt ? p.prompt.slice(0, 40) : null) ||
      id.slice(0, 6),
    model: p.model ?? null,
    effort: p.effort ?? null,
    auto_approve: false,
    match_snippet: null,
    last_activity_at: null,
    cache_cold: false,
    estimated_burst_tokens: null,
    hibernated: false,
    pinned: false,
    labels: [],
    last_heartbeat: null,
    intent: null,
    account_name: body.account ?? null,
    unread_count: 0,
    activity_detail: null,
    last_tool_at: null,
    last_tool_name: null,
    tool_use_count: 0,
    has_token_credentials: false,
  };
}

export function useSessionActions() {
  const qc = useQueryClient();
  const inval = () => qc.invalidateQueries({ queryKey: ["sessions"] });
  const invalLabels = () => qc.invalidateQueries({ queryKey: qk.labels });
  return {
    rename: async (id: string, name: string) => {
      await api.patch<void>(`/sessions/${id}`, { name });
      inval();
    },
    // Mark a session's messages seen (CCT-580). The caller invalidates the list
    // itself once the seen-mark lands, so this doesn't refetch on its own.
    markSeen: async (id: string) => {
      await endpoints.markSeen(id);
    },
    archive: async (id: string) => {
      await api.post<void>(`/sessions/${id}/archive`);
      inval();
    },
    unarchive: async (id: string) => {
      await api.post<void>(`/sessions/${id}/unarchive`);
      inval();
    },
    // Pin/unpin (CCT-267): pinned sessions sort to the top and are exempt
    // from auto-archive. Pinning an archived session also un-archives it.
    pin: async (id: string) => {
      await api.post<void>(`/sessions/${id}/pin`);
      inval();
    },
    unpin: async (id: string) => {
      await api.post<void>(`/sessions/${id}/unpin`);
      inval();
    },
    // Labels (CCT-360). `createLabel` is get-or-create by name (and recolors an
    // existing one); attach/detach wire a label to a session. Each mutation
    // refreshes the session list so the chips update in place.
    createLabel: async (name: string, color: string): Promise<Label> => {
      const label = await api.post<Label>("/labels", { name, color });
      invalLabels();
      return label;
    },
    // Edit a specific label in place (rename and/or recolor) — keyed on id, so
    // unlike `createLabel` it can rename without orphaning the old name.
    updateLabel: async (
      labelId: string,
      patch: { name?: string; color?: string },
    ): Promise<Label> => {
      const label = await api.patch<Label>(`/labels/${labelId}`, patch);
      invalLabels();
      inval();
      return label;
    },
    deleteLabel: async (labelId: string) => {
      await api.del<void>(`/labels/${labelId}`);
      invalLabels();
      inval();
    },
    attachLabel: async (id: string, labelId: string) => {
      await api.post<void>(`/sessions/${id}/labels`, { label_id: labelId });
      inval();
    },
    detachLabel: async (id: string, labelId: string) => {
      await api.del<void>(`/sessions/${id}/labels/${labelId}`);
      inval();
    },
    // Batch archive/unarchive (CCT-172). One request, one invalidation.
    archiveMany: async (ids: string[]) => {
      if (ids.length === 0) return;
      await api.post<void>("/sessions/archive", { ids });
      inval();
    },
    unarchiveMany: async (ids: string[]) => {
      if (ids.length === 0) return;
      await api.post<void>("/sessions/unarchive", { ids });
      inval();
    },
    kill: async (id: string) => {
      await api.post<void>(`/sessions/${id}/kill`);
      inval();
    },
    /** Stop the in-flight turn (CCT-210). Returns a `command_id` to await on
     *  the ws (CCT-339) so the caller can tell whether the agent actually
     *  accepted the interrupt instead of firing-and-forgetting. */
    interrupt: async (id: string) =>
      api.post<SpawnResponse>(`/sessions/${id}/interrupt`),
    // In-place model/effort switch (CCT-303). Codex carries it on the next
    // turn/start and echoes the resolved values back via Status; claude rejects
    // it (the UI offers fork-to-change-model for claude instead). Returns a
    // `command_id` to await on the ws (CCT-635) so the caller confirms the
    // change only once the adapter truthfully applied it.
    setModel: async (id: string, model?: string, effort?: string) => {
      const res = await api.post<SpawnResponse>(`/sessions/${id}/set-model`, {
        model: model || null,
        effort: effort || null,
      });
      inval();
      return res;
    },
    setAutoApprove: async (id: string, enabled: boolean) => {
      await api.post<void>(`/sessions/${id}/auto-approve`, { enabled });
      inval();
    },
    spawn: (body: SpawnRequest, files: File[] = []) =>
      endpoints.spawn(body, files),
    // Draft sessions (CCT-394): launch promotes a draft to a live spawn (env
    // entered fresh), discard deletes it. Both refetch the roster.
    launchDraft: async (id: string, env: Record<string, string> = {}) => {
      const res = await endpoints.launchDraft(id, env);
      inval();
      return res;
    },
    discardDraft: async (id: string) => {
      await endpoints.discardDraft(id);
      inval();
    },
    // Fork a conversation into a new session (CCT-302). Optionally overrides
    // model/effort (the "fork to change model" path for claude). The new
    // session links back to the parent and registers shortly after; refetch.
    fork: async (id: string, body: ForkRequest) => {
      const res = await endpoints.fork(id, body);
      inval();
      return res;
    },
    resume: async (id: string) => {
      await endpoints.resume(id);
      inval();
    },
    // Mid-chat attachments (CCT-236): stage files for a running session and
    // return the staged paths the composer references under the reply.
    stageFiles: (id: string, files: File[]) => endpoints.stageFiles(id, files),
    // Dispatch returns synchronously (no daemon ACK / command_id), so unlike
    // spawn there's nothing to await on the ws — the worker pod registers the
    // pre-minted session_id later. We optimistically insert a placeholder card
    // keyed by the client-minted session_id so the list updates IMMEDIATELY
    // (CCT-193); the eventual refetch reconciles it by id (the worker pod, or
    // the server's `failed` row on a backend error, both carry the same id).
    dispatch: async (body: DispatchRequest) => {
      const key = qk.sessions(false);
      const id = body.session_id ?? crypto.randomUUID();
      if (body.session_id == null) body = { ...body, session_id: id };
      const placeholder = optimisticDispatchCard(id, body);
      qc.setQueryData<SessionListResponse>(key, (prev) => ({
        sessions: [
          placeholder,
          ...(prev?.sessions ?? []).filter((s) => s.id !== id),
        ],
      }));
      try {
        const res = await endpoints.dispatch(body);
        inval();
        return res;
      } catch (e) {
        // Reconcile to server truth (the row exists as `failed`); the card
        // stays visible so the user can see + retry the failed dispatch.
        inval();
        throw e;
      }
    },
  };
}

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
    // Temporary on/off switch (CCT-251) — unlike revoke, nothing is
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
    // CCT-410: edit a user's ceiling (admin only). Re-intersects every key at
    // the next request; the server purges the auth cache so it's immediate.
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
    // token keeps working (CCT-410). Takes effect immediately (cache purge).
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

/** Enroll / rename / remove the caller's enrolled dispatchers (CCT-285).
 *  Invalidates both the management list and the merged dispatch picker. */
export function useDispatcherActions() {
  const qc = useQueryClient();
  const inval = () => {
    qc.invalidateQueries({ queryKey: ["user-dispatchers"] });
    qc.invalidateQueries({ queryKey: ["dispatchers"] });
  };
  return {
    enroll: async (body: { name: string; kind?: string; account?: string; provider?: string }) => {
      const r = await endpoints.enrollDispatcher(body);
      inval();
      return r;
    },
    rename: async (id: string, body: RenameDispatcher) => {
      const r = await endpoints.updateDispatcher(id, body);
      inval();
      return r;
    },
    remove: async (id: string) => {
      await endpoints.deleteDispatcher(id);
      inval();
    },
  };
}

/** CRUD for the caller's own OAuth accounts (CCT-237). Invalidates the accounts
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
    // "Sign in with Claude" (CCT-243): start returns the authorize URL the
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
    // Sharing (CCT-510): grant/revoke invalidate that account's shares query.
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
