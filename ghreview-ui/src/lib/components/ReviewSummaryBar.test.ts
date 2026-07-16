import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import ReviewSummaryBar from "./ReviewSummaryBar.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("ReviewSummaryBar", () => {
  it("exposes its full-width layout through the public prop", () => {
    component = mount(ReviewSummaryBar, {
      target: document.body,
      props: { draftCount: 3, fullWidth: true, onpublish: vi.fn() },
    });

    const bar = document.querySelector(".bar.full-width");
    const trigger = bar?.querySelector('[data-tsu="Popover"]');
    expect(bar).not.toBeNull();
    expect(trigger?.textContent).toContain("Review 3");
    expect(trigger?.classList.contains("trigger-sm")).toBe(true);
    expect(trigger?.classList.contains("trigger-primary")).toBe(false);
    expect(trigger?.classList.contains("trigger-tone-accent")).toBe(false);
    expect(trigger?.classList.contains("trigger-block")).toBe(true);
    expect(trigger?.querySelector("button")).toBeNull();
    expect(bar?.querySelector('[data-action="publish-review"]')?.getAttribute("data-tsu")).toBe(
      "Button",
    );
  });
});
