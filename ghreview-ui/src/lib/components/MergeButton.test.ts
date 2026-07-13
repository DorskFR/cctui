import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../api/client";
import type { GithubPull } from "../api/types";
import MergeButton from "./MergeButton.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

function pull(overrides: Partial<GithubPull> = {}): GithubPull {
  return {
    number: 42,
    title: "PR",
    state: "open",
    mergeable: true,
    head: { ref: "feat", sha: "sha1" },
    ...overrides,
  } as GithubPull;
}

async function openPanel(): Promise<void> {
  const trigger = document.querySelector(".trigger") as HTMLElement;
  trigger.click();
  await tick();
  await tick();
}

describe("MergeButton", () => {
  it("confirms then merges with the selected method and pinned head SHA", async () => {
    const spy = vi
      .spyOn(api, "mergePull")
      .mockResolvedValue({ merged: true, sha: "m1", message: "ok" });
    component = mount(MergeButton, {
      target: document.body,
      props: { owner: "o", repo: "r", number: 42, account: "acct", pull: pull() },
    });
    await openPanel();
    expect(document.body.textContent).toContain("Mergeable");

    (document.querySelector(".primary") as HTMLButtonElement).click();
    await tick();
    (document.querySelector(".confirm .primary") as HTMLButtonElement).click();
    await tick();
    await tick();

    expect(spy).toHaveBeenCalledWith("o", "r", 42, {
      account: "acct",
      merge_method: "squash",
      expected_head_sha: "sha1",
    });
  });

  it("shows a draft notice and offers no merge action for drafts", async () => {
    const spy = vi.spyOn(api, "mergePull");
    component = mount(MergeButton, {
      target: document.body,
      props: { owner: "o", repo: "r", number: 42, account: "acct", pull: pull({ draft: true }) },
    });
    await openPanel();
    expect(document.body.textContent).toContain("draft");
    expect(document.querySelector(".primary")).toBeNull();
    expect(spy).not.toHaveBeenCalled();
  });

  it("surfaces the API error message without merging away the panel", async () => {
    vi.spyOn(api, "mergePull").mockRejectedValue(new Error("not mergeable"));
    component = mount(MergeButton, {
      target: document.body,
      props: { owner: "o", repo: "r", number: 42, account: "acct", pull: pull() },
    });
    await openPanel();
    (document.querySelector(".primary") as HTMLButtonElement).click();
    await tick();
    (document.querySelector(".confirm .primary") as HTMLButtonElement).click();
    await tick();
    await tick();
    expect(document.querySelector(".err")?.textContent).toContain("not mergeable");
  });
});
