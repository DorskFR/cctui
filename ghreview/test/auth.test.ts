import { describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { loadConfig } from "../src/config.ts";

const resolver = createStaticResolver(parseStaticTokens("tok-a:user-a,tok-b:user-b"));

describe("auth middleware", () => {
  test("rejects a request with no bearer token", async () => {
    const app = createApp({ auth: resolver });
    const res = await app.request("/v1/repos");
    expect(res.status).toBe(401);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("unauthorized");
  });

  test("rejects an unknown token", async () => {
    const app = createApp({ auth: resolver });
    const res = await app.request("/v1/repos", { headers: { authorization: "Bearer nope" } });
    expect(res.status).toBe(401);
  });

  test("accepts a known token", async () => {
    const app = createApp({ auth: resolver });
    const res = await app.request("/v1/repos", { headers: { authorization: "Bearer tok-a" } });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ items: [], next_cursor: null });
  });

  test("no longer accepts an access_token query param, not even on the SSE route", async () => {
    const app = createApp({ auth: resolver });
    const res = await app.request("/v1/events?access_token=tok-a");
    expect(res.status).toBe(401);
  });

  test("requires auth for /v1/status and scopes it", async () => {
    const app = createApp({ auth: resolver });
    expect((await app.request("/v1/status")).status).toBe(401);
    const ok = await app.request("/v1/status", { headers: { authorization: "Bearer tok-a" } });
    expect(ok.status).toBe(200);
    const body = (await ok.json()) as { sync: { accounts: string[] } };
    expect(body.sync.accounts).toEqual([]);
  });

  test("leaves health, webhook and openapi exempt", async () => {
    const app = createApp({ auth: resolver });
    expect((await app.request("/v1/health")).status).toBe(200);
    expect((await app.request("/v1/openapi.json")).status).toBe(200);
  });

  test("parseStaticTokens ignores malformed pairs", () => {
    const map = parseStaticTokens("good:u1, bad, :missing, tok:u2");
    expect(map.get("good")).toBe("u1");
    expect(map.get("tok")).toBe("u2");
    expect(map.size).toBe(2);
  });
});

describe("fail-closed default", () => {
  test("denies non-exempt routes when no resolver is configured", async () => {
    const app = createApp();
    const res = await app.request("/v1/repos");
    expect(res.status).toBe(401);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("unauthorized");
  });

  test("keeps the exempt set servable without a resolver", async () => {
    const app = createApp();
    expect((await app.request("/v1/health")).status).toBe(200);
    expect((await app.request("/v1/openapi.json")).status).toBe(200);
    expect((await app.request("/v1/status")).status).toBe(401);
  });

  test("authDisabled opt-out serves non-exempt routes unauthenticated", async () => {
    const app = createApp({ authDisabled: true });
    const res = await app.request("/v1/repos");
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ items: [], next_cursor: null });
  });
});

describe("auth mode config", () => {
  test("static mode is available without a DATABASE_URL", () => {
    const cfg = loadConfig({ GHREVIEW_AUTH_MODE: "static", GHREVIEW_AUTH_TOKENS: "t:u" });
    expect(cfg.databaseUrl).toBeUndefined();
    expect(cfg.authMode).toBe("static");
  });

  test("static tokens authenticate without a store", async () => {
    const cfg = loadConfig({ GHREVIEW_AUTH_MODE: "static", GHREVIEW_AUTH_TOKENS: "t:u" });
    const app = createApp({ auth: createStaticResolver(parseStaticTokens(cfg.authTokens)) });
    const res = await app.request("/v1/repos", { headers: { authorization: "Bearer t" } });
    expect(res.status).toBe(200);
    expect((await app.request("/v1/repos")).status).toBe(401);
  });

  test("defaults to proxy and explicit none opt-out is honored", () => {
    expect(loadConfig({}).authMode).toBe("proxy");
    expect(loadConfig({ GHREVIEW_AUTH_MODE: "none" }).authMode).toBe("none");
  });

  test("the retired cctui mode is no longer a mode and needs no cctui schema", () => {
    const cfg = loadConfig({ GHREVIEW_AUTH_MODE: "cctui", GHREVIEW_CCTUI_SCHEMA: "public" });
    expect(cfg.authMode).toBe("proxy");
    expect("cctuiSchema" in cfg).toBe(false);
  });

  test("proxy mode reads its secret and skew window from the environment", () => {
    const cfg = loadConfig({ GHREVIEW_PROXY_SECRET: "s", GHREVIEW_PROXY_MAX_SKEW_SECONDS: "60" });
    expect(cfg.authMode).toBe("proxy");
    expect(cfg.proxySecret).toBe("s");
    expect(cfg.proxyMaxSkewSeconds).toBe(60);
    expect(loadConfig({}).proxyMaxSkewSeconds).toBe(300);
  });
});
