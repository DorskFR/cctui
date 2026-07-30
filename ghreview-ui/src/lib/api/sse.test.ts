import { describe, expect, it, vi } from "vitest";
import type { QueryClient } from "@tanstack/svelte-query";
import { keys } from "./queries";
import { applySseEvent, sseActions } from "./sse";
import type { SseEvent } from "./types";

describe("sseActions", () => {
  it("maps pr.updated to every view derived from the pull request", () => {
    const event: SseEvent = {
      event: "pr.updated",
      data: { account: "DorskFR", owner: "o", repo: "r", number: 7 },
    };
    expect(sseActions(event)).toEqual([
      { type: "invalidate", key: ["pull", "o", "r", 7] },
      { type: "invalidate", key: ["pull-viewed", "o", "r", 7] },
      { type: "invalidate", key: ["review-threads", "o", "r", 7] },
      { type: "invalidate", key: ["reviewers", "o", "r", 7] },
      { type: "invalidate", key: ["activity", "o", "r", 7] },
      { type: "invalidate", key: ["repo-labels", "o", "r"] },
      { type: "invalidate", key: ["pulls"] },
    ]);
  });

  it("invalidates account-scoped activity and label keys through their prefix", () => {
    const actions = sseActions({
      event: "pr.updated",
      data: { account: "DorskFR", owner: "o", repo: "r", number: 7 },
    });
    const invalidated = actions.map((a) => a.key);
    expect(invalidated).toContainEqual(keys.activityAll("o", "r", 7));
    expect(invalidated).toContainEqual(keys.repoLabelsAll("o", "r"));
    expect(keys.activity("o", "r", 7, "DorskFR").slice(0, 4)).toEqual(keys.activityAll("o", "r", 7));
    expect(keys.repoLabels("o", "r", "DorskFR").slice(0, 3)).toEqual(keys.repoLabelsAll("o", "r"));
  });

  it("maps pr.viewed_state.updated to the pull-viewed key", () => {
    const event: SseEvent = {
      event: "pr.viewed_state.updated",
      data: { account: "a", owner: "o", repo: "r", number: 7 },
    };
    expect(sseActions(event)).toEqual([
      { type: "invalidate", key: ["pull-viewed", "o", "r", 7] },
    ]);
  });

  it("maps notification events to the notifications key", () => {
    const nu: SseEvent = { event: "notification.updated", data: { account: "a", id: "1" } };
    const nn: SseEvent = { event: "notification.new", data: { account: "a", id: "2" } };
    expect(sseActions(nu)).toEqual([{ type: "invalidate", key: ["notifications"] }]);
    expect(sseActions(nn)).toEqual([{ type: "invalidate", key: ["notifications"] }]);
  });

  it("maps sync.status to the status key", () => {
    const event: SseEvent = {
      event: "sync.status",
      data: { account: "a", state: "syncing", last_run: null },
    };
    expect(sseActions(event)).toEqual([{ type: "invalidate", key: ["status"] }]);
  });
});

describe("applySseEvent", () => {
  it("invalidates every mapped key on the query client", () => {
    const invalidateQueries = vi.fn();
    const client = { invalidateQueries } as unknown as QueryClient;
    applySseEvent(client, {
      event: "pr.updated",
      data: { account: "a", owner: "o", repo: "r", number: 7 },
    });
    expect(invalidateQueries).toHaveBeenCalledTimes(7);
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["pull", "o", "r", 7] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["pull-viewed", "o", "r", 7] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["review-threads", "o", "r", 7] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["reviewers", "o", "r", 7] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["activity", "o", "r", 7] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["repo-labels", "o", "r"] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["pulls"] });
  });
});
