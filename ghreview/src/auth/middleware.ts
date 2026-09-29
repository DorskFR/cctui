import type { Context, Next } from "hono";
import type { ProxyAuthConfig } from "../deps.ts";
import {
  HEADER_PLUGIN,
  HEADER_SIG,
  HEADER_TS,
  HEADER_USER_ID,
  HEADER_USER_NAME,
  verifyProxyIdentity,
} from "./proxy.ts";
import type { AuthResolver } from "./resolver.ts";

export const USER_ID_KEY = "userId";

export const LOCAL_PRINCIPAL = "__local__";

export function getUserId(c: Context): string | undefined {
  const get = c.get as unknown as (k: string) => unknown;
  const v = get(USER_ID_KEY);
  return typeof v === "string" ? v : undefined;
}

export function setUserId(c: Context, id: string): void {
  const set = c.set as unknown as (k: string, v: unknown) => void;
  set(USER_ID_KEY, id);
}

function bearer(header: string | undefined): string | null {
  if (!header) return null;
  const match = /^Bearer\s+(.+)$/i.exec(header.trim());
  return match ? (match[1] as string).trim() : null;
}

/**
 * `GHREVIEW_AUTH_MODE=proxy`: the only credential is the signature cctui-server
 * puts on the request. No token reaches the browser, and ghreview never reads
 * cctui's database.
 */
export function proxyAuthMiddleware(config: ProxyAuthConfig) {
  return async (c: Context, next: Next) => {
    const result = verifyProxyIdentity({
      secret: config.secret,
      method: c.req.method,
      path: new URL(c.req.url).pathname,
      headers: {
        userId: c.req.header(HEADER_USER_ID),
        userName: c.req.header(HEADER_USER_NAME),
        plugin: c.req.header(HEADER_PLUGIN),
        ts: c.req.header(HEADER_TS),
        sig: c.req.header(HEADER_SIG),
      },
      maxSkewSeconds: config.maxSkewSeconds,
    });
    if (!result.ok) {
      const message =
        result.reason === "missing_headers"
          ? "Missing cctui proxy identity headers"
          : result.reason === "stale_timestamp"
            ? "cctui proxy identity timestamp is outside the accepted window"
            : "cctui proxy identity signature does not verify";
      return c.json({ error: { code: "unauthorized", message } }, 401);
    }
    setUserId(c, result.identity.userId);
    await next();
  };
}

export function authMiddleware(resolver: AuthResolver) {
  return async (c: Context, next: Next) => {
    const token = bearer(c.req.header("authorization"));
    if (!token) {
      return c.json({ error: { code: "unauthorized", message: "Missing bearer token" } }, 401);
    }
    let principal: Awaited<ReturnType<AuthResolver["resolve"]>>;
    try {
      principal = await resolver.resolve(token);
    } catch {
      return c.json({ error: { code: "internal", message: "Auth resolution failed" } }, 500);
    }
    if (!principal) {
      return c.json({ error: { code: "unauthorized", message: "Invalid bearer token" } }, 401);
    }
    setUserId(c, principal.userId);
    await next();
  };
}
