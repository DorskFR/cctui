import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import PrStateIcon, { type IconState, stateToneColor } from "./PrStateIcon.svelte";

let component: ReturnType<typeof mount> | undefined;

function render(props: { state: IconState; muted?: boolean; inherit?: boolean }): SVGElement {
  component = mount(PrStateIcon, { target: document.body, props });
  return document.querySelector("svg") as SVGElement;
}

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("stateToneColor", () => {
  it("maps every state onto the shared Tone scale", () => {
    expect(stateToneColor("open")).toBe("var(--ok)");
    expect(stateToneColor("merged")).toBe("var(--accent)");
    expect(stateToneColor("closed")).toBe("var(--danger)");
    expect(stateToneColor("draft")).toBe("var(--text-muted)");
    expect(stateToneColor("issue-open")).toBe("var(--ok)");
    expect(stateToneColor("issue-closed")).toBe("var(--accent)");
  });
});

describe("PrStateIcon", () => {
  it("renders a registry glyph rather than an inline path", () => {
    const icon = render({ state: "open" });

    expect(icon.getAttribute("data-tsu")).toBe("Icon");
    expect(icon.getAttribute("role")).toBe("img");
    expect(icon.getAttribute("aria-label")).toBe("open");
    expect(icon.getAttribute("style")).toContain("color: var(--ok)");
  });

  it("uses the merge glyph for a merged pull request", () => {
    const icon = render({ state: "merged" });

    expect(icon.getAttribute("aria-label")).toBe("merged");
    expect(icon.getAttribute("style")).toContain("color: var(--accent)");
  });

  it("tints the neutral draft state explicitly", () => {
    const icon = render({ state: "draft" });

    expect(icon.getAttribute("style")).toContain("color: var(--text-muted)");
  });

  it("mutes over the tone when asked", () => {
    const icon = render({ state: "open", muted: true });

    expect(icon.getAttribute("style")).toContain("color: var(--gh-fg-muted)");
    expect(icon.getAttribute("style")).not.toContain("var(--ok)");
  });

  it("inherits the surrounding colour with inherit", () => {
    const icon = render({ state: "closed", inherit: true });

    expect(icon.getAttribute("style")).not.toContain("color:");
    expect(icon.getAttribute("aria-label")).toBe("closed");
  });

  it("labels issue states readably", () => {
    expect(render({ state: "issue-open" }).getAttribute("aria-label")).toBe("issue open");
  });
});
