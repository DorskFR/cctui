import { describe, expect, it, vi } from "vitest";
import type { QueryClient } from "@tanstack/svelte-query";
import { applySseEvent, sseActions } from "./sse";
import type { SseEvent } from "./types";

describe("sseActions", () => {
  it("maps pr.updated to the PR envelope and the pull lists", () => {
    const event: SseEvent = {
      event: "pr.updated",
      data: { account: "DorskFR", owner: "o", repo: "r", number: 7 },
    };
    expect(sseActions(event)).toEqual([
      { type: "invalidate", key: ["pull", "o", "r", 7] },
      { type: "invalidate", key: ["pulls"] },
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
    expect(invalidateQueries).toHaveBeenCalledTimes(2);
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["pull", "o", "r", 7] });
    expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["pulls"] });
  });
});
