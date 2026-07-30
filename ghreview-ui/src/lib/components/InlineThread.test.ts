import { mount, unmount } from "svelte";
import { afterEach, describe, expect, it } from "vitest";
import type { ReviewThreadComment } from "../api/types";
import type { CommentAnchor } from "../review/anchors";
import InlineThread from "./InlineThread.svelte";

let component: ReturnType<typeof mount> | undefined;

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
});

function published(body: string): ReviewThreadComment {
  return {
    id: 11,
    path: "src/a.ts",
    line: 4,
    original_line: 4,
    side: "RIGHT",
    start_line: null,
    diff_hunk: null,
    body,
    user: "reviewer",
    in_reply_to_id: null,
    created_at: "2026-07-30T10:00:00Z",
    html_url: "https://github.com/example/project/pull/1#discussion_r11",
    reactions: null,
  };
}

function anchor(body: string): CommentAnchor {
  return {
    rowIndex: 3,
    path: "src/a.ts",
    side: "RIGHT",
    line: 4,
    drafts: [],
    published: [published(body)],
  };
}

function mountThread(body: string, props: Record<string, unknown> = {}): void {
  component = mount(InlineThread, {
    target: document.body,
    props: {
      anchor: anchor(body),
      onAdd: () => {},
      onEdit: () => {},
      onDelete: () => {},
      onClose: () => {},
      owner: "example",
      repo: "project",
      account: "reviewer",
      ...props,
    },
  });
}

describe("InlineThread markdown", () => {
  it("resolves relative links against the repository the thread belongs to", () => {
    mountThread("See [the guide](docs/guide.md)");

    expect(document.querySelector<HTMLAnchorElement>(".comment.published .body a")?.href).toBe(
      "https://github.com/example/project/docs/guide.md",
    );
  });

  it("drops relative links when no repository context is available", () => {
    mountThread("See [the guide](docs/guide.md)", { owner: undefined, repo: undefined });

    expect(document.querySelector(".comment.published .body a")).toBeNull();
    expect(document.querySelector(".comment.published .body")?.textContent).toContain("the guide");
  });
});
