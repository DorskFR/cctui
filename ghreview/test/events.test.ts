import { describe, expect, test } from "bun:test";
import { EventBus, mapNotice, type SseMessage } from "../src/events/bus.ts";

describe("mapNotice", () => {
  test("maps a pull_request notice to pr.updated with parsed coordinates", () => {
    const msg = mapNotice({ account: "DorskFR", kind: "pull_request", key: "DorskFR/cctui#42" });
    expect(msg).toEqual({
      event: "pr.updated",
      data: { account: "DorskFR", owner: "DorskFR", repo: "cctui", number: 42 },
    });
  });

  test("maps a notification notice to notification.new", () => {
    const msg = mapNotice({ account: "DorskFR", kind: "notification", key: "thread-9" });
    expect(msg).toEqual({
      event: "notification.new",
      data: { account: "DorskFR", id: "thread-9" },
    });
  });

  test("returns null for kinds without a catalogued event", () => {
    expect(mapNotice({ account: "DorskFR", kind: "repo", key: "DorskFR/cctui" })).toBeNull();
  });
});

describe("EventBus", () => {
  test("delivers published notices to subscribers as SSE messages", () => {
    const bus = new EventBus();
    const seen: SseMessage[] = [];
    const off = bus.subscribe((m) => seen.push(m));
    bus.publishNotice({ account: "DorskFR", kind: "pull_request", key: "DorskFR/cctui#7" });
    bus.publishSyncStatus("DorskFR", "idle", "2026-07-12T00:00:00Z");
    off();
    bus.publishNotice({ account: "DorskFR", kind: "notification", key: "t1" });
    expect(seen).toEqual([
      {
        event: "pr.updated",
        data: { account: "DorskFR", owner: "DorskFR", repo: "cctui", number: 7 },
      },
      {
        event: "sync.status",
        data: { account: "DorskFR", state: "idle", last_run: "2026-07-12T00:00:00Z" },
      },
    ]);
  });
});
