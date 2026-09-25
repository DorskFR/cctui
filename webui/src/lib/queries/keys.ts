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
  conversationHead: (id: string) => ["conversation-head", id] as const,
  messagePins: (id: string) => ["message-pins", id] as const,
  users: ["users"] as const,
  machines: (userId: string) => ["users", userId, "machines"] as const,
  tokens: (userId: string) => ["users", userId, "tokens"] as const,
  userAcls: (userId: string) => ["users", userId, "acls"] as const,
  userKeys: (userId: string) => ["users", userId, "keys"] as const,
  labels: ["labels"] as const,
  bookmarks: (q: string) => ["bookmarks", { q }] as const,
  accountShares: (accountId: string) => ["accounts", accountId, "shares"] as const,
  resourceShares: (resourceType: string, id: string) =>
    ["resource-shares", resourceType, id] as const,
  settingsCatalog: (family = "anthropic") =>
    ["settings-catalog", family] as const,
  machineResources: ["machine-resources"] as const,
  gitInfo: (machineId: string, path: string) =>
    ["machines", machineId, "gitinfo", path] as const,
  sessionsAll: ["sessions"] as const,
  conversationAll: ["conversation"] as const,
  me: ["me"] as const,
  accounts: ["accounts"] as const,
  redirects: ["redirects"] as const,
  toolPolicy: (accountId: string) => ["tool-policy", accountId] as const,
  accountPools: ["account-pools"] as const,
  accountPoolsUsage: ["account-pools-usage"] as const,
  sessionRebinds: (sessionId: string) => ["session-rebinds", sessionId] as const,
  usageHistory: (accountId: string | null, windowKey: string, from: string) =>
    ["account-usage-history", accountId, windowKey, from] as const,
  usageCloses: (from: string) => ["account-usage-closes", from] as const,
  user: (userId: string) => ["users", userId] as const,
  bookmarksAll: ["bookmarks"] as const,
  cacheLoss: (days: number) => ["cache-loss", { days }] as const,
  sessionDiagnose: (id: string) => ["session-diagnose", id] as const,
  sessionLangfuse: (id: string) => ["session-langfuse", id] as const,
  recentDirs: (machineId: string) => ["recent-dirs", machineId] as const,
  machineDirs: (machineId: string, path: string) =>
    ["machine-dirs", machineId, path] as const,
  codexModels: ["codex-models"] as const,
  codexModelsFor: (machineId: string) => ["codex-models", machineId] as const,
  codexModelsMerged: ["codex-models", "merged"] as const,
  sessionBindings: (sessionId: string) => ["session-bindings", sessionId] as const,
  dispatchers: ["dispatchers"] as const,
  userDispatchers: ["user-dispatchers"] as const,
  machinesAll: ["machines", "all"] as const,
  changelog: (version: string) => ["version", "changelog", version] as const,
  selfUpdateRun: ["version", "self-update-run"] as const,
};
