import { QueryClient } from "@tanstack/svelte-query";
import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { configureRuntime } from "../api/config";
import { keys } from "../api/queries";
import type { NotificationInboxItem, PullRequestEnvelope } from "../api/types";
import QueryHost from "../testing/QueryHost.svelte";
import Inbox from "./Inbox.svelte";

let component: ReturnType<typeof mount> | undefined;
let client: QueryClient;

function item(overrides: Partial<NotificationInboxItem> = {}): NotificationInboxItem {
  return {
    account: "acct",
    synced_at: "2026-07-30T10:00:00Z",
    state: { read: false, done: false, archived: false },
    payload: {
      id: "n1",
      reason: "review_requested",
      unread: true,
      updated_at: "2026-07-30T10:00:00Z",
      subject: {
        title: "Add tests",
        url: "https://api.github.com/repos/o/r/pulls/7",
        type: "PullRequest",
      },
      repository: { full_name: "o/r", name: "r" },
    },
    ...overrides,
  } as NotificationInboxItem;
}

function envelope(state: "open" | "closed", merged: boolean): PullRequestEnvelope {
  return {
    account: "acct",
    payload: { number: 7, title: "Add tests", state, merged },
  } as unknown as PullRequestEnvelope;
}

function mountInbox(): void {
  component = mount(QueryHost, {
    target: document.body,
    props: { client, component: Inbox },
  });
}

async function settleUntil(condition: () => boolean): Promise<void> {
  await vi.waitFor(() => {
    flushSync();
    if (!condition()) throw new Error("condition not met yet");
  });
}

beforeEach(() => {
  client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  configureRuntime({ baseUrl: "https://ghreview.example", token: null, account: "acct" });
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.replaceChildren();
  client.clear();
  configureRuntime(null);
  vi.restoreAllMocks();
});

describe("Inbox", () => {
  it("renders one row body per notification through the shared snippet", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ items: [item()], next_cursor: null }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );

    mountInbox();
    await settleUntil(() => document.querySelectorAll(".list li").length === 1);

    expect(document.querySelectorAll(".list li")).toHaveLength(1);
    expect(document.querySelectorAll(".list li .body")).toHaveLength(1);
    expect(document.querySelectorAll(".list li .subject")).toHaveLength(1);
    expect(document.querySelector(".list li .reason")?.textContent).toBe("review_requested");
  });

  it("picks up the pull state as soon as the pull lands in the query cache", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ items: [item()], next_cursor: null }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );

    mountInbox();
    await settleUntil(() => document.querySelector(".list li svg") !== null);

    expect(document.querySelector(".list li svg")?.getAttribute("aria-label")).toBe("open");
    expect(document.querySelector(".list li svg")?.getAttribute("fill")).toBe(
      "var(--gh-fg-muted)",
    );

    client.setQueryData(keys.pull("o", "r", 7), envelope("closed", true));
    await settleUntil(
      () => document.querySelector(".list li svg")?.getAttribute("aria-label") === "merged",
    );

    expect(document.querySelector(".list li svg")?.getAttribute("aria-label")).toBe("merged");
  });

  it("re-queries with the account injected after mount", async () => {
    const fetchMock = vi.fn(
      async (_input: string | URL | Request, _init?: RequestInit) =>
        new Response(JSON.stringify({ items: [], next_cursor: null }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
    );
    vi.stubGlobal("fetch", fetchMock);

    mountInbox();
    await settleUntil(() => fetchMock.mock.calls.length === 1);
    expect(fetchMock.mock.calls.map(([url]) => String(url))).toEqual([
      "https://ghreview.example/v1/notifications?account=acct&all=true",
    ]);

    configureRuntime({ baseUrl: "https://ghreview.example", token: null, account: "other" });
    await settleUntil(() => fetchMock.mock.calls.length === 2);

    expect(fetchMock.mock.calls.map(([url]) => String(url))).toContain(
      "https://ghreview.example/v1/notifications?account=other&all=true",
    );
  });

  it("renders the empty inbox as an EmptyState, not ad-hoc muted text", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(
        async () =>
          new Response(JSON.stringify({ items: [], next_cursor: null }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          }),
      ),
    );

    mountInbox();
    await settleUntil(() => document.querySelector('[data-tsu="EmptyState"]') !== null);

    const empty = document.querySelector('[data-tsu="EmptyState"]');
    expect(empty?.textContent).toContain("Inbox zero.");
    expect(document.querySelector(".msg")).toBeNull();
    expect(document.querySelector(".list")).toBeNull();
  });

  it("renders a load failure as a danger Callout", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response("nope", { status: 500 })),
    );

    mountInbox();
    await settleUntil(() => document.querySelector('[data-tsu="Callout"]') !== null);

    const callout = document.querySelector('[data-tsu="Callout"]') as HTMLElement;
    expect(callout.getAttribute("role")).toBe("alert");
    expect(callout.className).toContain("callout-danger");
    expect(document.querySelector(".err")).toBeNull();
  });
});
