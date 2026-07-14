import { mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Review from "../../Review.svelte";
import { configureRuntime } from "../api/config";
import { queryClient } from "../api/queries";
import { router } from "../router/router.svelte";

class MockEventSource {
  addEventListener(): void {}
  close(): void {}
}

const WIDTH_KEY = "ghreview:masterWidth";
let component: ReturnType<typeof mount> | undefined;

async function renderMasterDetail(): Promise<void> {
  component = mount(Review, {
    target: document.body,
    props: {
      baseUrl: "https://review.example",
      token: "session-token",
      basePath: "",
    },
  });
  await tick();
  await tick();
}

beforeEach(() => {
  vi.stubGlobal("EventSource", MockEventSource);
  vi.stubGlobal(
    "fetch",
    vi.fn(
      async () =>
        new Response(JSON.stringify({ items: [], next_cursor: null }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    ),
  );
  localStorage.clear();
  queryClient.clear();
  router.navigate("/", true);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  queryClient.clear();
  configureRuntime(null);
  vi.restoreAllMocks();
});

describe("MasterDetail", () => {
  it("renders list and detail content in the configured resizable panel", async () => {
    await renderMasterDetail();

    const layout = document.querySelector('[data-tsu="ResizablePanel"]') as HTMLElement;
    const separator = layout.querySelector('[role="separator"]');

    expect(layout).not.toBeNull();
    expect(layout.classList.contains("left")).toBe(true);
    expect(layout.classList.contains("right")).toBe(false);
    expect(layout.classList.contains("collapsed")).toBe(false);
    expect(layout.getAttribute("style")).toContain("--panel-width: 320px");
    expect(separator?.getAttribute("aria-valuenow")).toBe("320");
    expect(localStorage.getItem(WIDTH_KEY)).toBeNull();
    expect(layout.querySelector('aside[aria-label="Pull request list"] .wrap')).not.toBeNull();
    expect(separator?.getAttribute("aria-label")).toBe("Resize Pull request list");
    expect(separator?.getAttribute("aria-valuemin")).toBe("220");
    expect(separator?.getAttribute("aria-valuemax")).toBe("720");
    expect(layout.querySelector(".detail .detail-bar .tabbar")).not.toBeNull();
    expect(layout.querySelector(".detail .detail-body")?.textContent).toContain(
      "Select a pull request",
    );
  });

  it("restores and persists collapsed state and expanded width", async () => {
    localStorage.setItem(WIDTH_KEY, "410");
    localStorage.setItem(`${WIDTH_KEY}:collapsed`, "true");
    await renderMasterDetail();

    const layout = document.querySelector('[data-tsu="ResizablePanel"]') as HTMLElement;
    const toggle = layout.querySelector(
      'button[aria-label="Expand Pull request list"]',
    ) as HTMLButtonElement;

    expect(layout.classList.contains("collapsed")).toBe(true);
    expect(layout.getAttribute("style")).toContain("--panel-width: 410px");
    expect(layout.querySelector(".detail")).not.toBeNull();
    expect(layout.querySelector(".wrap")).toBeNull();

    toggle.click();
    await tick();
    expect(localStorage.getItem(`${WIDTH_KEY}:collapsed`)).toBe("false");

    const separator = layout.querySelector('[role="separator"]') as HTMLElement;
    separator.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await tick();
    expect(layout.getAttribute("style")).toContain("--panel-width: 434px");
    expect(localStorage.getItem(WIDTH_KEY)).toBe("434");

    const collapse = layout.querySelector(
      'button[aria-label="Collapse Pull request list"]',
    ) as HTMLButtonElement;
    collapse.click();
    await tick();
    expect(localStorage.getItem(`${WIDTH_KEY}:collapsed`)).toBe("true");
  });
});
