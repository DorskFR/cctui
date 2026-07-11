import { describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { signPayload, verifySignature } from "../src/github/webhook.ts";

const SECRET = "s3cr3t";

describe("verifySignature", () => {
  test("accepts a correct sha256 HMAC", () => {
    const body = JSON.stringify({ hello: "world" });
    expect(verifySignature(SECRET, body, signPayload(SECRET, body))).toBe(true);
  });

  test("rejects a tampered body", () => {
    const sig = signPayload(SECRET, "original");
    expect(verifySignature(SECRET, "tampered", sig)).toBe(false);
  });

  test("rejects a missing signature", () => {
    expect(verifySignature(SECRET, "x", null)).toBe(false);
  });

  test("rejects a wrong secret", () => {
    const body = "payload";
    expect(verifySignature("other", body, signPayload(SECRET, body))).toBe(false);
  });
});

describe("POST /v1/webhook", () => {
  const app = createApp({ webhookSecret: SECRET });

  test("401s an invalid signature", async () => {
    const res = await app.request("/v1/webhook", {
      method: "POST",
      headers: { "x-hub-signature-256": "sha256=deadbeef", "x-github-event": "ping" },
      body: "{}",
    });
    expect(res.status).toBe(401);
  });

  test("202s a valid signature", async () => {
    const body = JSON.stringify({ zen: "keep it simple" });
    const res = await app.request("/v1/webhook", {
      method: "POST",
      headers: { "x-hub-signature-256": signPayload(SECRET, body), "x-github-event": "ping" },
      body,
    });
    expect(res.status).toBe(202);
  });

  test("503s when no secret is configured", async () => {
    const bare = createApp();
    const res = await bare.request("/v1/webhook", { method: "POST", body: "{}" });
    expect(res.status).toBe(503);
  });
});
