export type AuthMode = "cctui" | "static" | "none";

export interface Config {
  databaseUrl: string | undefined;
  schema: string;
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
  cctuiSchema: string;
  syncViewedFromGithub: boolean;
}

function parseAuthMode(value: string | undefined): AuthMode {
  if (value === "static") return "static";
  if (value === "none") return "none";
  return "cctui";
}

function num(value: string | undefined, fallback: number): number {
  if (value === undefined || value.trim() === "") return fallback;
  const n = Number(value);
  return Number.isFinite(n) ? n : fallback;
}

export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  return {
    databaseUrl: env.DATABASE_URL,
    schema: env.GHREVIEW_SCHEMA ?? "ghreview",
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
    cctuiSchema: env.GHREVIEW_CCTUI_SCHEMA ?? "public",
    syncViewedFromGithub: env.GHREVIEW_SYNC_VIEWED_GITHUB === "true",
  };
}
