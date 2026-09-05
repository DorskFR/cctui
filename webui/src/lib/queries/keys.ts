/** Machine kinds the server manages itself — the per-user `dispatch` machine
 * and one-shot `ephemeral` worker pods. They are never spawn targets and are
 * hidden from the "new machines" list in the UI. */
export const SYSTEM_MACHINE_KINDS = new Set(["dispatch", "ephemeral"]);

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
  sessionAttachments: (id: string) => ["session-attachments", id] as const,
  users: ["users"] as const,
  machines: (userId: string) => ["users", userId, "machines"] as const,
  tokens: (userId: string) => ["users", userId, "tokens"] as const,
  userAcls: (userId: string) => ["users", userId, "acls"] as const,
  userKeys: (userId: string) => ["users", userId, "keys"] as const,
  labels: ["labels"] as const,
  accountShares: (accountId: string) => ["accounts", accountId, "shares"] as const,
  resourceShares: (resourceType: string, id: string) =>
    ["resource-shares", resourceType, id] as const,
  settingsCatalog: ["settings-catalog"] as const,
  gitInfo: (machineId: string, path: string) =>
    ["machines", machineId, "gitinfo", path] as const,
};
