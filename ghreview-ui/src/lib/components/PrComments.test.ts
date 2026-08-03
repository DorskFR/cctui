import { QueryClient } from "@tanstack/svelte-query";
import { mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ActivityEvent, ReviewThreadComment } from "../api/types";
import QueryHost from "../testing/QueryHost.svelte";
import PrComments, { buildCommentGroups, commentViewState, reviewLabel } from "./PrComments.svelte";

function event(input: Partial<ActivityEvent> & Pick<ActivityEvent, "event">): ActivityEvent {
  return {
    id: null,
    actor: null,
    created_at: null,
    html_url: null,
    reactions: null,
    ...input,
  } as ActivityEvent;
}

function inline(
  id: number,
  created_at: string,
  in_reply_to_id: number | null = null,
): ReviewThreadComment {
  return {
    id,
    path: "src/a.ts",
    line: 4,
    original_line: 4,
    side: "RIGHT",
    start_line: null,
    diff_hunk: null,
    body: `inline ${id}`,
    user: "reviewer",
    in_reply_to_id,
    created_at,
    html_url: `https://github.com/o/r/pull/1#discussion_r${id}`,
    reactions: null,
  };
}

describe("buildCommentGroups", () => {
  it("combines issue comments, reviews, and inline replies chronologically", () => {
    const groups = buildCommentGroups(
      [
        event({
          id: "review-1",
          event: "reviewed",
          actor: { login: "ada", avatar_url: "avatar" },
          created_at: "2026-07-14T02:00:00Z",
          detail: { state: "APPROVED", body: "looks good" },
        }),
        event({
          id: "comment-1",
          event: "commented",
          actor: { login: "lin", avatar_url: null },
          created_at: "2026-07-14T01:00:00Z",
          detail: { body: "question" },
        }),
        event({ event: "committed", created_at: "2026-07-14T00:00:00Z" }),
      ],
      [
        inline(11, "2026-07-14T03:00:00Z"),
        inline(12, "2026-07-14T04:00:00Z", 11),
      ],
    );

    expect(groups).toHaveLength(3);
    expect(groups.map((group) => group.entries[0]?.kind)).toEqual(["issue", "review", "inline"]);
    expect(groups[2]?.entries.map((entry) => entry.id)).toEqual([11, 12]);
    expect(groups[1]?.entries[0]).toMatchObject({ author: "ada", reviewState: "APPROVED" });
  });
});

describe("comment view state", () => {
  it("distinguishes loading, error, empty, and partial content", () => {
    expect(commentViewState({ loading: true, error: null, groups: [] })).toBe("no-account");
    expect(commentViewState({ account: "a", loading: true, error: null, groups: [] })).toBe(
      "loading",
    );
    expect(
      commentViewState({ account: "a", loading: false, error: new Error("failed"), groups: [] }),
    ).toBe("error");
    expect(commentViewState({ account: "a", loading: false, error: null, groups: [] })).toBe(
      "empty",
    );
    expect(
      commentViewState({
        account: "a",
        loading: true,
        error: new Error("partial"),
        groups: [{ key: "one", entries: [] }],
      }),
    ).toBe("content");
  });

  it("formats review verdicts", () => {
    expect(reviewLabel("APPROVED")).toBe("Approved");
    expect(reviewLabel("CHANGES_REQUESTED")).toBe("Requested changes");
  });
});

describe("PrComments rendering", () => {
  let component: ReturnType<typeof mount> | undefined;
  let client: QueryClient;

  beforeEach(() => {
    client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ items: [] }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );
  });

  afterEach(async () => {
    if (component) await unmount(component);
    component = undefined;
    document.body.replaceChildren();
    client.clear();
    vi.restoreAllMocks();
  });

  function mountComments(body: string, owner = "example", repo = "project"): void {
    const comment = { ...inline(21, "2026-07-30T10:00:00Z"), body };
    component = mount(QueryHost, {
      target: document.body,
      props: {
        client,
        component: PrComments,
        props: { owner, repo, number: 1, account: "reviewer", inline: [comment] },
      },
    });
  }

  it("resolves relative comment links against the pull request's repository", () => {
    mountComments("See [the guide](docs/guide.md)");

    expect(document.querySelector<HTMLAnchorElement>(".comments .body a")?.href).toBe(
      "https://github.com/example/project/docs/guide.md",
    );
  });

  it("keeps absolute links untouched", () => {
    mountComments("See [upstream](https://example.com/page)");

    expect(document.querySelector<HTMLAnchorElement>(".comments .body a")?.href).toBe(
      "https://example.com/page",
    );
  });
});
