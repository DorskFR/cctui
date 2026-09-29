import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { configureRuntime } from "./lib/api/config";
import { queryClient } from "./lib/api/queries";
import type { HostContext } from "./lib/plugin/host";
import { router } from "./lib/router/router.svelte";
import PluginHost from "./lib/testing/PluginHost.svelte";
import ReviewPage from "./ReviewPage.svelte";

class MockEventSource {
  static urls: string[] = [];
  constructor(url: string) {
    MockEventSource.urls.push(url);
  }
  addEventListener(): void {}
  close(): void {}
}

let component: ReturnType<typeof mount> | undefined;

function ok(): Response {
  return new Response(JSON.stringify({ items: [], next_cursor: null }), {
    status: 200,
    headers: { "Content-Type": "application/json" },
  });
}

async function render(host: Partial<HostContext>, path = "/"): Promise<void> {
  component = mount(PluginHost, {
    target: document.body,
    props: {
      host: { cctuiApi: 1, origin: "https://cctui.example", ...host } as HostContext,
      component: ReviewPage,
      props: { basePath: "/apps/ghreview", path, navigate: host.navigate ?? (() => {}) },
    },
  });
  await tick();
  await tick();
  await tick();
}

beforeEach(() => {
  MockEventSource.urls = [];
  vi.stubGlobal("EventSource", MockEventSource);
  localStorage.clear();
  queryClient.clear();
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  queryClient.clear();
  configureRuntime(null);
  router.adopt(null);
  vi.restoreAllMocks();
});

describe("ReviewPage on a host without pluginFetch", () => {
  it("refuses to run rather than falling back to a direct fetch", async () => {
    await render({});

    expect(document.body.textContent).toContain("too old");
    expect(document.querySelector('[data-tsu="Callout"]')).not.toBeNull();
    expect(MockEventSource.urls).toEqual([]);
  });
});

describe("ReviewPage backend gate", () => {
  it("reports an unconfigured or unreachable backend and offers a retry", async () => {
    const pluginFetch = vi
      .fn<(path: string, init?: RequestInit) => Promise<Response>>()
      .mockImplementationOnce(async () => new Response("no upstream", { status: 503 }));

    await render({ pluginFetch });

    expect(pluginFetch).toHaveBeenCalledWith("/v1/health");
    expect(document.body.textContent).toContain("not configured or not reachable");
    expect(document.body.textContent).toContain("503");
    expect(document.querySelector("button")?.textContent).toContain("Retry");
  });

  it("treats a thrown proxy error as unreachable and shows the reason", async () => {
    const pluginFetch = vi
      .fn<(path: string, init?: RequestInit) => Promise<Response>>()
      .mockRejectedValueOnce(new Error("network down"));

    await render({ pluginFetch });

    expect(document.body.textContent).toContain("not configured or not reachable");
    expect(document.body.textContent).toContain("network down");
  });

  it("retries the probe and mounts the shell once the backend answers", async () => {
    const pluginFetch = vi
      .fn<(path: string, init?: RequestInit) => Promise<Response>>()
      .mockImplementationOnce(async () => new Response("no upstream", { status: 503 }))
      .mockImplementation(async () => ok());

    await render({ pluginFetch });
    expect(document.body.textContent).toContain("not configured or not reachable");

    (document.querySelector("button") as HTMLButtonElement).click();
    await tick();
    await tick();
    await tick();

    expect(document.querySelector("header.toolbar")).not.toBeNull();
  });
});

describe("ReviewPage routing and transport", () => {
  it("takes its route from the host path and streams through the proxy", async () => {
    const pluginFetch = vi
      .fn<(path: string, init?: RequestInit) => Promise<Response>>()
      .mockImplementation(async () => ok());

    await render({ pluginFetch }, "/inbox");

    expect(router.current.name).toBe("inbox");
    expect(MockEventSource.urls).toEqual(["/api/v1/plugins/ghreview/backend/v1/events"]);
  });

  it("hands navigations back to the host instead of touching the address bar", async () => {
    const navigate = vi.fn<(path: string) => void>();
    const pluginFetch = vi
      .fn<(path: string, init?: RequestInit) => Promise<Response>>()
      .mockImplementation(async () => ok());
    const before = window.location.pathname;

    await render({ pluginFetch, navigate }, "/");
    router.navigate("/subscriptions");
    await tick();

    expect(navigate).toHaveBeenCalledWith("/subscriptions");
    expect(router.current.name).toBe("subscriptions");
    expect(window.location.pathname).toBe(before);
  });
});
