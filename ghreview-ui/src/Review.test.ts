import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import Review from "./Review.svelte";
import { configureRuntime } from "./lib/api/config";

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
  it("renders the shell and scopes the theme to its container", async () => {
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
    expect(container?.getAttribute("data-theme")).toBeTruthy();
    expect(document.body.textContent).toContain("gh-review");
  });
});
