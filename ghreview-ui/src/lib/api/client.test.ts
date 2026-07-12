import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "./client";
import { configureRuntime } from "./config";

afterEach(() => {
  configureRuntime(null);
  vi.restoreAllMocks();
});

describe("api client with injected runtime auth", () => {
  it("prefixes the injected baseUrl and sends the injected bearer token", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: "session-token" });
    const fetchMock = vi.fn(
      async () =>
        new Response(JSON.stringify({ ok: true }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await api.status();

    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe("https://ghreview.example/v1/status");
    const headers = new Headers(init.headers);
    expect(headers.get("Authorization")).toBe("Bearer session-token");
  });

  it("omits the Authorization header when the injected token is null", async () => {
    configureRuntime({ baseUrl: "https://ghreview.example", token: null });
    const fetchMock = vi.fn(
      async () => new Response(null, { status: 204 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await api.status();

    const [, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    const headers = new Headers(init.headers);
    expect(headers.get("Authorization")).toBeNull();
  });
});
