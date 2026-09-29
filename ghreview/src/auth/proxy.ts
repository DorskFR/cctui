import { createHmac, timingSafeEqual } from "node:crypto";

export const HEADER_USER_ID = "x-cctui-user-id";
export const HEADER_USER_NAME = "x-cctui-user-name";
export const HEADER_PLUGIN = "x-cctui-plugin";
export const HEADER_TS = "x-cctui-ts";
export const HEADER_SIG = "x-cctui-sig";

export const DEFAULT_MAX_SKEW_SECONDS = 300;

export interface ProxyIdentity {
  userId: string;
  userName: string;
  plugin: string;
}

export type ProxyRejection =
  | "missing_headers"
  | "stale_timestamp"
  | "bad_signature";

/**
 * The canonical string cctui-server signs. Must stay byte-identical to
 * `canonical()` in cctui's `plugin_proxy.rs`; the shared vectors in
 * `docs/plugin-proxy-signature-vectors.json` pin both.
 */
export function canonical(method: string, path: string, ts: number, userId: string): string {
  return `${method}\n${path}\n${ts}\n${userId}`;
}

export function sign(
  secret: string,
  method: string,
  path: string,
  ts: number,
  userId: string,
): string {
  return createHmac("sha256", secret).update(canonical(method, path, ts, userId)).digest("hex");
}

/** The path the signature covers: one leading slash, no query or fragment. */
export function signedPath(rawPath: string): string {
  const withoutQuery = rawPath.split(/[?#]/, 1)[0] ?? "";
  return `/${withoutQuery.replace(/^\/+/, "")}`;
}

function constantTimeEquals(a: string, b: string): boolean {
  const left = Buffer.from(a, "utf8");
  const right = Buffer.from(b, "utf8");
  if (left.length !== right.length) return false;
  return timingSafeEqual(left, right);
}

export interface VerifyInput {
  secret: string;
  method: string;
  path: string;
  headers: {
    userId: string | undefined;
    userName: string | undefined;
    plugin: string | undefined;
    ts: string | undefined;
    sig: string | undefined;
  };
  nowSeconds?: number;
  maxSkewSeconds?: number;
}

export type VerifyResult =
  | { ok: true; identity: ProxyIdentity }
  | { ok: false; reason: ProxyRejection };

export function verifyProxyIdentity(input: VerifyInput): VerifyResult {
  const { userId, userName, plugin, ts, sig } = input.headers;
  if (!userId || !ts || !sig) return { ok: false, reason: "missing_headers" };

  const timestamp = Number(ts);
  if (!Number.isInteger(timestamp)) return { ok: false, reason: "stale_timestamp" };
  const now = input.nowSeconds ?? Math.floor(Date.now() / 1000);
  const skew = input.maxSkewSeconds ?? DEFAULT_MAX_SKEW_SECONDS;
  if (Math.abs(now - timestamp) > skew) return { ok: false, reason: "stale_timestamp" };

  const expected = sign(input.secret, input.method, signedPath(input.path), timestamp, userId);
  if (!constantTimeEquals(expected, sig)) return { ok: false, reason: "bad_signature" };

  return { ok: true, identity: { userId, userName: userName ?? "", plugin: plugin ?? "" } };
}
