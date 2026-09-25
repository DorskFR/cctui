// @vitest-environment happy-dom
import { describe, expect, it, vi } from "vitest";
import type { AgentEvent } from "@bindings/AgentEvent";
import { CONVERSATION_FETCH_LIMIT } from "$lib/queries";
import { EarlierPages, FOCUS_CONTEXT } from "./earlierPages.svelte";

const ev = (seq: number) =>
  ({ seq, ts: seq, type: "text" }) as unknown as AgentEvent;
const page = (from: number, n: number) =>
  Array.from({ length: n }, (_, i) => ev(from + i));

function make(
  over: Partial<{ id: string; history: number; events: AgentEvent[] }> = {},
) {
  let id = over.id ?? "s1";
  const fetch = vi.fn(
    async (_sid: string, q: { limit: number; before: number }) =>
      page(q.before - q.limit, q.limit),
  );
  const pages: EarlierPages = new EarlierPages({
    id: () => id,
    historyLength: () => over.history ?? CONVERSATION_FETCH_LIMIT,
    events: () => [...pages.pages, ...(over.events ?? [ev(500)])],
    fetch,
  });
  return { pages, fetch, setId: (v: string) => (id = v) };
}

describe("EarlierPages", () => {
  it("offers older pages only when the tail page is full or pages were fetched", () => {
    expect(make({ history: 3 }).pages.canFetch).toBe(false);
    expect(make().pages.canFetch).toBe(true);
  });

  it("prepends a full page before the oldest known seq and keeps offering more", async () => {
    const { pages, fetch } = make();
    await pages.fetchEarlier();
    expect(fetch).toHaveBeenCalledWith("s1", {
      limit: CONVERSATION_FETCH_LIMIT,
      before: 500,
    });
    expect(pages.pages).toHaveLength(CONVERSATION_FETCH_LIMIT);
    expect(pages.pages[0].seq).toBe(500 - CONVERSATION_FETCH_LIMIT);
    expect(pages.canFetch).toBe(true);
    await pages.fetchEarlier();
    expect(fetch).toHaveBeenLastCalledWith("s1", {
      limit: CONVERSATION_FETCH_LIMIT,
      before: 500 - CONVERSATION_FETCH_LIMIT,
    });
  });

  it("marks the head once a short page comes back", async () => {
    const { pages, fetch } = make();
    fetch.mockResolvedValueOnce([ev(499)]);
    await pages.fetchEarlier();
    expect(pages.exhausted).toBe(true);
    expect(pages.canFetch).toBe(false);
    await pages.fetchEarlier();
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it("drops a page that lands after the session changed", async () => {
    const { pages, fetch, setId } = make();
    let release!: (v: AgentEvent[]) => void;
    fetch.mockReturnValueOnce(new Promise<AgentEvent[]>((r) => (release = r)));
    const p = pages.fetchEarlier();
    setId("s2");
    release(page(0, CONVERSATION_FETCH_LIMIT));
    await p;
    expect(pages.pages).toEqual([]);
    expect(pages.fetching).toBe(false);
  });

  it("fetches one focus window per session+seq and flags a short one as the head", async () => {
    const { pages, fetch } = make();
    await pages.focus("s1", 100);
    expect(fetch).toHaveBeenCalledWith("s1", {
      limit: FOCUS_CONTEXT * 2,
      before: 100 + FOCUS_CONTEXT,
    });
    expect(pages.exhausted).toBe(false);
    await pages.focus("s1", 100);
    expect(fetch).toHaveBeenCalledTimes(1);
    fetch.mockResolvedValueOnce([ev(1)]);
    await pages.focus("s1", 7);
    expect(pages.exhausted).toBe(true);
  });

  it("reset forgets pages and the head flag", async () => {
    const { pages, fetch } = make();
    fetch.mockResolvedValueOnce([ev(499)]);
    await pages.fetchEarlier();
    pages.reset();
    expect(pages.pages).toEqual([]);
    expect(pages.exhausted).toBe(false);
  });
});
