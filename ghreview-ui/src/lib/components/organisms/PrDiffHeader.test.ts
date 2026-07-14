import { QueryClient } from "@tanstack/svelte-query";
import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../../api/client";
import type { GithubFile, GithubPull } from "../../api/types";
import PrDiffHeader from "./PrDiffHeader.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

const pull: GithubPull = {
  number: 17,
  title: "Improve diff navigation",
  state: "open",
  mergeable: true,
  additions: 24,
  deletions: 8,
  ci: "success",
  user: { login: "contributor" },
  base: { ref: "main", sha: "base-sha" },
  head: { ref: "feature/diff-nav", sha: "head-sha" },
  labels: [{ name: "enhancement", color: "1f6feb" }],
  html_url: "https://github.com/example/project/pull/17",
};

const files = [
  {
    filename: "src/diff.ts",
    status: "modified",
    additions: 24,
    deletions: 8,
    changes: 32,
  },
] as GithubFile[];

function render(overrides: Partial<GithubPull> = {}): void {
  vi.spyOn(api, "reviewers").mockResolvedValue({ reviewers: [], requested_teams: [] });
  component = mount(PrDiffHeader, {
    target: document.body,
    context: new Map([["$$_queryClient", new QueryClient()]]),
    props: {
      owner: "example",
      repo: "project",
      number: 17,
      account: "contributor",
      pull: { ...pull, ...overrides },
      files,
      viewedCount: 1,
      draftCount: 2,
      onpublish: vi.fn(),
    },
  });
}

describe("PrDiffHeader", () => {
  it("presents state, identity, statistics, source, and diff actions in semantic groups", () => {
    render();

    const header = document.querySelector("header.pr-diff-header");
    const heading = header?.querySelector("h1");
    const titleLink = heading?.querySelector("a");
    const stats = header?.querySelector('[aria-label="Pull request statistics"]');
    const actions = header?.querySelector('[aria-label="Pull request actions"]');

    expect(header).not.toBeNull();
    expect(header?.querySelector(".identity")?.firstElementChild?.textContent).toContain("open");
    expect(heading?.textContent).toBe("Improve diff navigation");
    expect(titleLink?.getAttribute("href")).toBe("https://github.com/example/project/pull/17");
    expect(header?.querySelector(".number")?.textContent).toBe("#17");
    expect(header?.querySelector('[aria-label="Pull request labels"]')?.textContent).toContain(
      "enhancement",
    );
    expect(stats?.textContent).toContain("+24");
    expect(stats?.textContent).toContain("−8");
    expect(stats?.textContent).toContain("1 files");
    expect(stats?.textContent).toContain("viewed 1/1");
    expect(stats?.textContent).toContain("CI success");
    expect(stats?.textContent).toContain("mergeable");
    expect(header?.querySelector(".author")?.textContent).toContain("contributor");
    expect(header?.querySelector(".branches")?.textContent).toContain("main");
    expect(header?.querySelector(".branches")?.textContent).toContain("feature/diff-nav");
    expect(actions?.querySelector('[role="radiogroup"]')?.getAttribute("aria-label")).toBe(
      "Diff layout",
    );
    expect(actions?.textContent).toContain("Review 2");
    expect(actions?.textContent).toContain("Merge");
    expect(actions?.querySelector(".bar.full-width")).not.toBeNull();
    expect(actions?.querySelector(".merge-button.full-width")).not.toBeNull();
    expect(actions?.querySelector('[role="radiogroup"]')?.classList.contains("seg-sm")).toBe(true);
    const popoverTriggers = actions?.querySelectorAll('[data-tsu="Popover"]');
    expect(popoverTriggers?.length).toBe(2);
    expect(popoverTriggers?.[0].classList.contains("trigger-sm")).toBe(true);
    expect(popoverTriggers?.[0].classList.contains("trigger-primary")).toBe(true);
    expect(popoverTriggers?.[0].classList.contains("trigger-tone-accent")).toBe(true);
    expect(popoverTriggers?.[1].classList.contains("trigger-sm")).toBe(true);
    expect(popoverTriggers?.[1].classList.contains("trigger-primary")).toBe(true);
    expect(popoverTriggers?.[1].classList.contains("trigger-tone-success")).toBe(true);
    expect(actions?.querySelector(".trigger")).toBeNull();
  });

  it("keeps diff selection interactive and exposes the narrow-screen stacking hooks", async () => {
    render();

    const unified = document.querySelector('[role="radio"][aria-label="Unified"]');
    const split = document.querySelector('[role="radio"][aria-label="Split"]') as HTMLButtonElement;
    expect(unified?.getAttribute("aria-checked")).toBe("true");

    split.click();
    await tick();

    expect(split.getAttribute("aria-checked")).toBe("true");
    expect(document.querySelector(".identity h1")).not.toBeNull();
    expect(document.querySelector(".source .branches")).not.toBeNull();
    expect(document.querySelector(".reviewers-row")).not.toBeNull();
    expect(document.querySelector(".actions .diff-mode")).not.toBeNull();
    expect(document.querySelector(".actions .review-action")).not.toBeNull();
  });
});
