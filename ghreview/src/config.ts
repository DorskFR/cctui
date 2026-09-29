export type AuthMode = "proxy" | "static" | "none";

export interface Config {
  databaseUrl: string | undefined;
  githubToken: string | undefined;
  githubAccount: string | undefined;
  pollIntervalMs: number;
  budgetCeilingFraction: number;
  rateLimitPerHour: number;
  webhookSecret: string | undefined;
  port: number;
  sealKey: string | undefined;
  authMode: AuthMode;
  authTokens: string | undefined;
  unsafeAllowAnonymous: boolean;
  proxySecret: string | undefined;
  proxyMaxSkewSeconds: number;
  syncViewedFromGithub: boolean;
}

function parseAuthMode(value: string | undefined): AuthMode {
  if (value === "static") return "static";
  if (value === "none") return "none";
  return "proxy";
}

function num(value: string | undefined, fallback: number): number {
  if (value === undefined || value.trim() === "") return fallback;
  const n = Number(value);
  return Number.isFinite(n) ? n : fallback;
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  return {
    databaseUrl: env.DATABASE_URL,
    githubToken: env.GITHUB_TOKEN,
    githubAccount: env.GITHUB_ACCOUNT,
    pollIntervalMs: num(env.GHREVIEW_POLL_INTERVAL_MS, 30_000),
    budgetCeilingFraction: num(env.GHREVIEW_BUDGET_CEILING, 0.2),
    rateLimitPerHour: num(env.GHREVIEW_RATE_LIMIT, 5000),
    webhookSecret: env.GHREVIEW_WEBHOOK_SECRET,
    port: num(env.PORT, 8790),
    sealKey: env.GHREVIEW_SEAL_KEY,
    authMode: parseAuthMode(env.GHREVIEW_AUTH_MODE),
    authTokens: env.GHREVIEW_AUTH_TOKENS,
    unsafeAllowAnonymous: env.GHREVIEW_UNSAFE_ALLOW_ANONYMOUS === "true",
    proxySecret: env.GHREVIEW_PROXY_SECRET,
    proxyMaxSkewSeconds: num(env.GHREVIEW_PROXY_MAX_SKEW_SECONDS, 300),
    syncViewedFromGithub: env.GHREVIEW_SYNC_VIEWED_GITHUB === "true",
  };
}
