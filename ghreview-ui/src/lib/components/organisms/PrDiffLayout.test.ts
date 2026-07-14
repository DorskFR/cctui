import { createRawSnippet, mount, tick, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { GithubFile } from "../../api/types";
import { buildDiffModel } from "../../diff/parse";
import PrDiffLayout from "./PrDiffLayout.svelte";

const WIDTH_KEY = "ghreview:filesWidth";
const file: GithubFile = {
  filename: "src/example.ts",
  status: "modified",
  additions: 1,
  deletions: 1,
  changes: 2,
  patch: "@@ -1 +1 @@\n-before\n+after",
};
const model = buildDiffModel([file]);
const content = createRawSnippet(() => ({ render: () => "<div data-diff>Diff content</div>" }));
let component: ReturnType<typeof mount> | undefined;

async function renderLayout(): Promise<void> {
  component = mount(PrDiffLayout, {
    target: document.body,
    props: {
      model,
      focusRow: 0,
      viewed: new Set<string>(),
      onselect: vi.fn(),
      onToggleViewed: vi.fn(),
      children: content,
    },
  });
  await tick();
  await tick();
}

beforeEach(() => localStorage.clear());

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("PrDiffLayout", () => {
  it("places the changed-files tree in the configured left panel", async () => {
    await renderLayout();

    const layout = document.querySelector('[data-tsu="ResizablePanel"]') as HTMLElement;
    const separator = layout.querySelector('[role="separator"]');

    expect(layout.classList.contains("left")).toBe(true);
    expect(layout.classList.contains("right")).toBe(false);
    expect(layout.classList.contains("collapsed")).toBe(false);
    expect(layout.getAttribute("style")).toContain("--panel-width: 280px");
    expect(layout.querySelector('aside[aria-label="Changed files"]')).not.toBeNull();
    expect(layout.querySelector(".tree")?.textContent).toContain("example.ts");
    expect(layout.querySelector("[data-diff]")?.textContent).toBe("Diff content");
    expect(separator?.getAttribute("aria-label")).toBe("Resize Changed files");
    expect(separator?.getAttribute("aria-valuemin")).toBe("220");
    expect(separator?.getAttribute("aria-valuemax")).toBe("640");
    expect(separator?.getAttribute("aria-valuenow")).toBe("280");
    expect(localStorage.getItem(WIDTH_KEY)).toBeNull();
  });

  it("restores collapse and width, then persists left-side keyboard resizing", async () => {
    localStorage.setItem(WIDTH_KEY, "400");
    localStorage.setItem(`${WIDTH_KEY}:collapsed`, "true");
    await renderLayout();

    const layout = document.querySelector('[data-tsu="ResizablePanel"]') as HTMLElement;
    const expand = layout.querySelector(
      'button[aria-label="Expand Changed files"]',
    ) as HTMLButtonElement;

    expect(layout.classList.contains("collapsed")).toBe(true);
    expect(layout.getAttribute("style")).toContain("--panel-width: 400px");
    expect(layout.querySelector(".tree")).toBeNull();

    expand.click();
    await tick();
    expect(localStorage.getItem(`${WIDTH_KEY}:collapsed`)).toBe("false");

    const separator = layout.querySelector('[role="separator"]') as HTMLElement;
    separator.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await tick();
    expect(layout.getAttribute("style")).toContain("--panel-width: 424px");
    expect(localStorage.getItem(WIDTH_KEY)).toBe("424");

    separator.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    await tick();
    expect(layout.getAttribute("style")).toContain("--panel-width: 400px");

    const collapse = layout.querySelector(
      'button[aria-label="Collapse Changed files"]',
    ) as HTMLButtonElement;
    collapse.click();
    await tick();
    expect(localStorage.getItem(`${WIDTH_KEY}:collapsed`)).toBe("true");
  });

  it("resizes from the left panel edge with pointer input and persists on release", async () => {
    await renderLayout();

    const layout = document.querySelector('[data-tsu="ResizablePanel"]') as HTMLElement;
    const separator = layout.querySelector('[role="separator"]') as HTMLElement & {
      setPointerCapture: (pointerId: number) => void;
      releasePointerCapture: (pointerId: number) => void;
    };
    vi.spyOn(layout, "getBoundingClientRect").mockReturnValue({
      left: 0,
      right: 800,
      top: 0,
      bottom: 600,
      width: 800,
      height: 600,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    });
    separator.setPointerCapture = vi.fn();
    separator.releasePointerCapture = vi.fn();

    function pointer(type: string, clientX: number): MouseEvent {
      const event = new MouseEvent(type, { bubbles: true, clientX });
      Object.defineProperty(event, "pointerId", { value: 7 });
      return event;
    }

    separator.dispatchEvent(pointer("pointerdown", 280));
    separator.dispatchEvent(pointer("pointermove", 480));
    expect(localStorage.getItem(WIDTH_KEY)).toBeNull();

    separator.dispatchEvent(pointer("pointerup", 480));
    await tick();
    expect(layout.getAttribute("style")).toContain("--panel-width: 480px");
    expect(localStorage.getItem(WIDTH_KEY)).toBe("480");
    expect(separator.setPointerCapture).toHaveBeenCalledWith(7);
    expect(separator.releasePointerCapture).toHaveBeenCalledWith(7);
  });
});
