import type { Context, Next } from "hono";
import type { AuthResolver } from "./resolver.ts";

export const USER_ID_KEY = "userId";

export function getUserId(c: Context): string | undefined {
  const get = c.get as unknown as (k: string) => unknown;
  const v = get(USER_ID_KEY);
  return typeof v === "string" ? v : undefined;
}

function setUserId(c: Context, id: string): void {
  const set = c.set as unknown as (k: string, v: unknown) => void;
  set(USER_ID_KEY, id);
}

function bearer(header: string | undefined): string | null {
  if (!header) return null;
  const match = /^Bearer\s+(.+)$/i.exec(header.trim());
  return match ? (match[1] as string).trim() : null;
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
