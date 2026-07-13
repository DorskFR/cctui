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
    expect(bar).not.toBeNull();
    expect(bar?.querySelector("button")?.textContent).toContain("Review 3");
  });
});
