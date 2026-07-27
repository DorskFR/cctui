const TOKEN_KEY = "ghreview:token";
const ACCOUNT_KEY = "ghreview:account";

// Runtime config injected by an embedder (cctui-ui); when set it wins over
// the standalone localStorage / VITE_* sources. null = standalone default.
export interface GhreviewRuntimeConfig {
  baseUrl?: string;
  token?: string | null;
  account?: string | null;
  basePath?: string;
}

let runtime: GhreviewRuntimeConfig | null = null;

export function configureRuntime(config: GhreviewRuntimeConfig | null): void {
  runtime = config;
}

export function isEmbedded(): boolean {
  return runtime !== null;
}

export function baseUrl(): string {
  if (runtime?.baseUrl != null) return runtime.baseUrl.replace(/\/$/, "");
  const env = import.meta.env.VITE_GHREVIEW_URL;
  if (env) return env.replace(/\/$/, "");
  return "";
}

export function basePath(): string {
  return (runtime?.basePath ?? "").replace(/\/$/, "");
}

export function getToken(): string | null {
  if (runtime && "token" in runtime) return runtime.token ?? null;
  const stored = localStorage.getItem(TOKEN_KEY);
  if (stored) return stored;
  return import.meta.env.VITE_GHREVIEW_TOKEN ?? null;
}

export function setToken(token: string | null): void {
  if (token) localStorage.setItem(TOKEN_KEY, token);
  else localStorage.removeItem(TOKEN_KEY);
}

export function getAccount(): string | null {
  if (runtime && "account" in runtime) return runtime.account ?? null;
  const stored = localStorage.getItem(ACCOUNT_KEY);
  if (stored) return stored;
  return import.meta.env.VITE_GHREVIEW_ACCOUNT ?? null;
}

export function setAccount(account: string | null): void {
  if (account) localStorage.setItem(ACCOUNT_KEY, account);
  else localStorage.removeItem(ACCOUNT_KEY);
}
