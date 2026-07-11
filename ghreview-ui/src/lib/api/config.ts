const TOKEN_KEY = "ghreview:token";
const ACCOUNT_KEY = "ghreview:account";

export function baseUrl(): string {
  const env = import.meta.env.VITE_GHREVIEW_URL;
  if (env) return env.replace(/\/$/, "");
  return "";
}

export function getToken(): string | null {
  const stored = localStorage.getItem(TOKEN_KEY);
  if (stored) return stored;
  return import.meta.env.VITE_GHREVIEW_TOKEN ?? null;
}

export function setToken(token: string | null): void {
  if (token) localStorage.setItem(TOKEN_KEY, token);
  else localStorage.removeItem(TOKEN_KEY);
}

export function getAccount(): string | null {
  const stored = localStorage.getItem(ACCOUNT_KEY);
  if (stored) return stored;
  return import.meta.env.VITE_GHREVIEW_ACCOUNT ?? null;
}

export function setAccount(account: string | null): void {
  if (account) localStorage.setItem(ACCOUNT_KEY, account);
  else localStorage.removeItem(ACCOUNT_KEY);
}
