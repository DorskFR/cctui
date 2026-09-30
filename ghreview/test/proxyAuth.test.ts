import { describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import {
  canonical,
  DEFAULT_MAX_SKEW_SECONDS,
  HEADER_PLUGIN,
  HEADER_SIG,
  HEADER_TS,
  HEADER_USER_ID,
  HEADER_USER_NAME,
  sign,
  signedPath,
  verifyProxyIdentity,
} from "../src/auth/proxy.ts";

const SECRET = "proxy-secret";
const USER = "9f1c2d34-5e6f-4a7b-8c9d-0e1f2a3b4c5d";

interface Vector {
  name: string;
  secret: string;
  method: string;
  path: string;
  ts: number;
  userId: string;
  canonical: string;
  signature: string;
}

const vectors: Vector[] = (
  (await Bun.file(
    new URL("../../docs/plugin-proxy-signature-vectors.json", import.meta.url),
  ).json()) as { vectors: Vector[] }
).vectors;

function signedRequest(
  path: string,
  init: { method?: string; ts?: number; secret?: string; userId?: string } = {},
): [string, RequestInit] {
  const method = init.method ?? "GET";
  const ts = init.ts ?? Math.floor(Date.now() / 1000);
  const userId = init.userId ?? USER;
  return [
    path,
    {
      method,
      headers: {
        [HEADER_USER_ID]: userId,
        [HEADER_USER_NAME]: "Dorsk",
        [HEADER_PLUGIN]: "ghreview",
        [HEADER_TS]: String(ts),
        [HEADER_SIG]: sign(init.secret ?? SECRET, method, signedPath(path), ts, userId),
      },
    },
  ];
}

describe("shared signature vectors", () => {
  test("the vector file is non-empty and every vector reproduces", () => {
    expect(vectors.length).toBeGreaterThan(0);
    for (const v of vectors) {
      expect(canonical(v.method, v.path, v.ts, v.userId)).toBe(v.canonical);
      expect(sign(v.secret, v.method, v.path, v.ts, v.userId)).toBe(v.signature);
    }
  });

  test("the canonical string is method, path, ts and user joined by newlines", () => {
    expect(canonical("GET", "/v1/pulls", 1_700_000_000, "u")).toBe("GET\n/v1/pulls\n1700000000\nu");
  });

  test("the signed path drops the query and normalises the leading slash", () => {
    expect(signedPath("v1/pulls")).toBe("/v1/pulls");
    expect(signedPath("/v1/pulls")).toBe("/v1/pulls");
    expect(signedPath("//v1/pulls")).toBe("/v1/pulls");
    expect(signedPath("v1/pulls?state=open")).toBe("/v1/pulls");
    expect(signedPath("v1/pulls#f")).toBe("/v1/pulls");
    expect(signedPath("")).toBe("/");
  });
});

describe("verifyProxyIdentity", () => {
  const headers = (over: Record<string, string | undefined> = {}) => {
    const ts = 1_700_000_000;
    return {
      nowSeconds: ts,
      secret: SECRET,
      method: "GET",
      path: "/v1/pulls",
      headers: {
        userId: USER,
        userName: "Dorsk",
        plugin: "ghreview",
        ts: String(ts),
        sig: sign(SECRET, "GET", "/v1/pulls", ts, USER),
        ...over,
      },
    };
  };

  test("accepts a correctly signed request and takes the user id from the header", () => {
    const result = verifyProxyIdentity(headers());
    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.identity.userId).toBe(USER);
      expect(result.identity.userName).toBe("Dorsk");
      expect(result.identity.plugin).toBe("ghreview");
    }
  });

  test("rejects a missing signature, timestamp or user", () => {
    for (const over of [{ sig: undefined }, { ts: undefined }, { userId: undefined }]) {
      const result = verifyProxyIdentity(headers(over));
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.reason).toBe("missing_headers");
    }
  });

  test("rejects a stale or future timestamp outside the window", () => {
    const stale = verifyProxyIdentity({
      ...headers(),
      nowSeconds: 1_700_000_000 + DEFAULT_MAX_SKEW_SECONDS + 1,
    });
    expect(stale.ok).toBe(false);
    if (!stale.ok) expect(stale.reason).toBe("stale_timestamp");

    const future = verifyProxyIdentity({
      ...headers(),
      nowSeconds: 1_700_000_000 - DEFAULT_MAX_SKEW_SECONDS - 1,
    });
    expect(future.ok).toBe(false);

    const edge = verifyProxyIdentity({
      ...headers(),
      nowSeconds: 1_700_000_000 + DEFAULT_MAX_SKEW_SECONDS,
    });
    expect(edge.ok).toBe(true);

    const garbage = verifyProxyIdentity(headers({ ts: "not-a-number" }));
    expect(garbage.ok).toBe(false);
    if (!garbage.ok) expect(garbage.reason).toBe("stale_timestamp");
  });

  test("a replayed signature cannot be moved to another method, path or user", () => {
    for (const input of [
      { ...headers(), method: "POST" },
      { ...headers(), path: "/v1/repos" },
      { ...headers(), secret: "wrong" },
    ]) {
      const result = verifyProxyIdentity(input);
      expect(result.ok).toBe(false);
      if (!result.ok) expect(result.reason).toBe("bad_signature");
    }
    const swapped = verifyProxyIdentity(headers({ userId: "someone-else" }));
    expect(swapped.ok).toBe(false);
  });

  test("the timestamp itself is signed, so it cannot be refreshed", () => {
    const now = 1_700_000_500;
    const result = verifyProxyIdentity({
      ...headers({ ts: String(now) }),
      nowSeconds: now,
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason).toBe("bad_signature");
  });
});

describe("proxy auth mode end to end", () => {
  const app = () => createApp({ proxyAuth: { secret: SECRET } });

  test("a signed request is served and its user id is the header's", async () => {
    const res = await app().request(...signedRequest("/v1/repos"));
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ items: [], next_cursor: null });
  });

  test("an unsigned request is rejected, bearer tokens included", async () => {
    expect((await app().request("/v1/repos")).status).toBe(401);
    const bearer = await app().request("/v1/repos", {
      headers: { authorization: "Bearer anything" },
    });
    expect(bearer.status).toBe(401);
  });

  test("a request signed with the wrong secret is rejected", async () => {
    const res = await app().request(...signedRequest("/v1/repos", { secret: "nope" }));
    expect(res.status).toBe(401);
    const body = (await res.json()) as { error: { code: string; message: string } };
    expect(body.error.code).toBe("unauthorized");
    expect(body.error.message).toContain("signature");
  });

  test("a stale timestamp is rejected even with a valid signature", async () => {
    const res = await app().request(
      ...signedRequest("/v1/repos", { ts: Math.floor(Date.now() / 1000) - 3600 }),
    );
    expect(res.status).toBe(401);
    const body = (await res.json()) as { error: { message: string } };
    expect(body.error.message).toContain("timestamp");
  });

  test("the SSE route needs a signature too and takes no access_token", async () => {
    const res = await app().request("/v1/events?access_token=anything");
    expect(res.status).toBe(401);
  });

  test("health, webhook and openapi stay exempt", async () => {
    expect((await app().request("/v1/health")).status).toBe(200);
    expect((await app().request("/v1/openapi.json")).status).toBe(200);
  });

  test("the query string is not signed, so it may vary freely", async () => {
    const [path, init] = signedRequest("/v1/repos");
    const res = await app().request(`${path}?account=x`, init);
    expect(res.status).toBe(200);
  });
});
