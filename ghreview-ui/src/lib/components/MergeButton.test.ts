import { mount, tick, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import { api } from "../api/client";
import { queryClient } from "../api/queries";
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
  it("exposes its full-width layout through the public prop", () => {
    component = mount(MergeButton, {
      target: document.body,
      props: {
        owner: "o",
        repo: "r",
        number: 42,
        account: "acct",
        pull: pull(),
        fullWidth: true,
      },
    });

    expect(document.querySelector(".merge-button.full-width")).not.toBeNull();
    expect(document.querySelector(".merge-button .trigger")?.textContent).toBe("Merge");
  });

  it("confirms then merges with the selected method and pinned head SHA", async () => {
    const onmerged = vi.fn();
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const spy = vi
      .spyOn(api, "mergePull")
      .mockResolvedValue({ merged: true, sha: "m1", message: "ok" });
    component = mount(MergeButton, {
      target: document.body,
      props: { owner: "o", repo: "r", number: 42, account: "acct", pull: pull(), onmerged },
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
    expect(onmerged).toHaveBeenCalledOnce();
    expect(invalidate).toHaveBeenCalledWith({ queryKey: ["pulls"] });
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
    const onmerged = vi.fn();
    vi.spyOn(api, "mergePull").mockRejectedValue(new Error("not mergeable"));
    component = mount(MergeButton, {
      target: document.body,
      props: { owner: "o", repo: "r", number: 42, account: "acct", pull: pull(), onmerged },
    });
    await openPanel();
    (document.querySelector(".primary") as HTMLButtonElement).click();
    await tick();
    (document.querySelector(".confirm .primary") as HTMLButtonElement).click();
    await tick();
    await tick();
    expect(document.querySelector(".err")?.textContent).toContain("not mergeable");
    expect(document.querySelector(".confirm")).not.toBeNull();
    expect(onmerged).not.toHaveBeenCalled();
  });

  it("keeps the view open when the API declines the merge", async () => {
    const onmerged = vi.fn();
    vi.spyOn(api, "mergePull").mockResolvedValue({
      merged: false,
      sha: null,
      message: "required checks are pending",
    });
    component = mount(MergeButton, {
      target: document.body,
      props: { owner: "o", repo: "r", number: 42, account: "acct", pull: pull(), onmerged },
    });
    await openPanel();
    (document.querySelector(".primary") as HTMLButtonElement).click();
    await tick();
    (document.querySelector(".confirm .primary") as HTMLButtonElement).click();
    await tick();
    await tick();

    expect(document.querySelector(".err")?.textContent).toContain("required checks are pending");
    expect(document.querySelector(".confirm")).not.toBeNull();
    expect(onmerged).not.toHaveBeenCalled();
  });
});
