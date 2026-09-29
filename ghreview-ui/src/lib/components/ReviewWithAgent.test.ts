import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { GithubPull } from "../api/types";
import type { HostContext, HostSpawnRequest } from "../plugin/host";
import PluginHost from "../testing/PluginHost.svelte";
import ReviewWithAgent from "./ReviewWithAgent.svelte";

let component: ReturnType<typeof mount> | undefined;

const pull = {
  number: 46,
  title: "Add the review plugin",
  state: "open",
  head: { ref: "lane/p3", sha: "cafebabe" },
  base: { ref: "main", sha: "f00d" },
  html_url: "https://github.com/DorskFR/cctui/pull/46",
} as GithubPull;

function render(host: Partial<HostContext>): void {
  component = mount(PluginHost, {
    target: document.body,
    props: {
      host: { cctuiApi: 1, origin: "https://cctui.example", ...host } as HostContext,
      component: ReviewWithAgent,
      props: { owner: "DorskFR", repo: "cctui", pull },
    },
  });
}

function action(): HTMLButtonElement | null {
  return document.querySelector('[data-action="review-with-agent"]');
}

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

describe("ReviewWithAgent", () => {
  it("is hidden when the host cannot open the spawn form", () => {
    render({});

    expect(action()).toBeNull();
    expect(document.body.textContent).not.toContain("Review with agent");
  });

  it("hands the host a prompt naming the pull request and its head SHA", () => {
    const openSpawn = vi.fn<(request: HostSpawnRequest) => void>();
    render({ openSpawn });

    const button = action();
    expect(button?.textContent).toContain("Review with agent");
    button?.click();

    expect(openSpawn).toHaveBeenCalledTimes(1);
    const [request] = openSpawn.mock.calls[0];
    expect(request.prompt).toContain("DorskFR/cctui#46");
    expect(request.prompt).toContain("cafebabe");
    expect(request.prompt).toContain("https://github.com/DorskFR/cctui/pull/46");
    expect(request.working_dir).toBeUndefined();
    expect(request.machine_id).toBeUndefined();
  });
});
