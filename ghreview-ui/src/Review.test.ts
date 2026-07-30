import { flushSync, mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import Review from "./Review.svelte";
import { baseUrl, configureRuntime, getToken, isEmbedded } from "./lib/api/config";

class MockEventSource {
  addEventListener(): void {}
  close(): void {}
}

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  configureRuntime(null);
  vi.restoreAllMocks();
});

describe("Review (embedded mount)", () => {
  it("renders the shell and inherits the host theme (no local data-theme)", async () => {
    vi.stubGlobal("EventSource", MockEventSource);
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }),
      ),
    );

    component = mount(Review, {
      target: document.body,
      props: {
        baseUrl: "https://ghreview.example",
        token: "session-token",
        basePath: "/review",
      },
    });

    const container = document.querySelector(".ghreview-embed");
    expect(container).not.toBeNull();
    expect(container?.hasAttribute("data-theme")).toBe(false);
    expect(document.body.textContent).toContain("Pull requests");
  });

  it("releases the injected runtime config when the embed unmounts", async () => {
    vi.stubGlobal("EventSource", MockEventSource);
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }),
      ),
    );

    const mounted = mount(Review, {
      target: document.body,
      props: { baseUrl: "https://ghreview.example", token: "session-token" },
    });
    flushSync();
    expect(isEmbedded()).toBe(true);
    expect(baseUrl()).toBe("https://ghreview.example");

    await unmount(mounted);

    expect(isEmbedded()).toBe(false);
    expect(baseUrl()).toBe("");
    expect(getToken()).toBeNull();
  });
});
