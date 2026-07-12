import { describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";

const app = createApp();

async function spec() {
  const res = await app.request("/v1/openapi.json");
  return res.json() as Promise<{
    openapi: string;
    paths: Record<string, unknown>;
    components: { schemas: Record<string, unknown> };
  }>;
}

describe("openapi contract", () => {
  test("is a versioned 3.0.3 document", async () => {
    const s = await spec();
    expect(s.openapi).toBe("3.0.3");
  });

  test("exposes the /v1 route surface", async () => {
    const s = await spec();
    expect(Object.keys(s.paths).sort()).toEqual([
      "/v1/accounts",
      "/v1/accounts/{id}",
      "/v1/events",
      "/v1/github/repos",
      "/v1/health",
      "/v1/notifications",
      "/v1/notifications/state",
      "/v1/notifications/{id}/state",
      "/v1/repos",
      "/v1/repos/{owner}/{repo}",
      "/v1/repos/{owner}/{repo}/pulls",
      "/v1/repos/{owner}/{repo}/pulls/{number}",
      "/v1/repos/{owner}/{repo}/pulls/{number}/viewed",
      "/v1/status",
      "/v1/subscriptions",
      "/v1/subscriptions/{id}",
      "/v1/webhook",
    ]);
  });

  test("documents the envelope, page, error and event schemas", async () => {
    const names = Object.keys((await spec()).components.schemas);
    for (const expected of [
      "RepoEnvelope",
      "PullRequestEnvelope",
      "RepoPage",
      "PullRequestPage",
      "NotificationInboxItem",
      "NotificationInboxPage",
      "NotificationState",
      "NotificationStateItem",
      "NotificationStateResult",
      "NotificationBulkState",
      "NotificationSingleState",
      "AccountCreate",
      "AccountSummary",
      "AccountList",
      "Error",
      "SseEvent",
      "PrUpdatedEvent",
      "NotificationNewEvent",
      "NotificationUpdatedEvent",
      "SyncStatusEvent",
    ]) {
      expect(names).toContain(expected);
    }
  });
});

describe("route shapes", () => {
  test("health returns ok", async () => {
    const res = await app.request("/v1/health");
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
  });

  test("status carries service + sync fields", async () => {
    const res = await app.request("/v1/status");
    const body = (await res.json()) as Record<string, unknown>;
    expect(body.service).toBe("gh-review");
    expect(body.api).toBe("v1");
    expect(body.sync).toEqual({ last_run: null, accounts: [] });
  });

  test("list endpoints return an empty page envelope", async () => {
    for (const path of ["/v1/repos", "/v1/repos/DorskFR/cctui/pulls", "/v1/notifications"]) {
      const res = await app.request(path);
      expect(res.status).toBe(200);
      expect(await res.json()).toEqual({ items: [], next_cursor: null });
    }
  });

  test("detail endpoints return the error envelope when unsynced", async () => {
    const res = await app.request("/v1/repos/DorskFR/cctui/pulls/42");
    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("not_found");
  });

  test("rejects out-of-range pagination with the error model", async () => {
    const res = await app.request("/v1/repos?limit=9999");
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("invalid_request");
  });

  test("unknown routes use the error envelope", async () => {
    const res = await app.request("/v1/nope");
    expect(res.status).toBe(404);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("not_found");
  });
});
