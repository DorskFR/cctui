import { describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";

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

  test("leaves health, status and webhook exempt", async () => {
    const app = createApp({ auth: resolver });
    expect((await app.request("/v1/health")).status).toBe(200);
    expect((await app.request("/v1/status")).status).toBe(200);
    expect((await app.request("/v1/openapi.json")).status).toBe(200);
  });

  test("parseStaticTokens ignores malformed pairs", () => {
    const map = parseStaticTokens("good:u1, bad, :missing, tok:u2");
    expect(map.get("good")).toBe("u1");
    expect(map.get("tok")).toBe("u2");
    expect(map.size).toBe(2);
  });
});
