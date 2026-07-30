import { describe, expect, test } from "bun:test";
import { EventBus, mapNotice, type SseMessage } from "../src/events/bus.ts";
import { EventQueue } from "../src/events/queue.ts";

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

  test("maps a notification_state notice to notification.updated", () => {
    const msg = mapNotice({ account: "DorskFR", kind: "notification_state", key: "thread-9" });
    expect(msg).toEqual({
      event: "notification.updated",
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

describe("EventQueue", () => {
  test("drains everything queued and empties itself", () => {
    const q = new EventQueue<number>(10);
    q.push(1);
    q.push(2);
    expect(q.size).toBe(2);
    expect(q.drain()).toEqual([1, 2]);
    expect(q.size).toBe(0);
    expect(q.drain()).toEqual([]);
  });

  test("drops the oldest entries past the cap and counts them", () => {
    const q = new EventQueue<number>(3);
    for (const n of [1, 2, 3, 4, 5]) q.push(n);
    expect(q.size).toBe(3);
    expect(q.drain()).toEqual([3, 4, 5]);
    expect(q.takeDropped()).toBe(2);
    expect(q.takeDropped()).toBe(0);
  });

  test("wait resolves immediately when items are already queued", async () => {
    const q = new EventQueue<number>(10);
    q.push(1);
    expect(await q.wait(60_000)).toBe(true);
  });

  test("wait wakes on push rather than polling", async () => {
    const q = new EventQueue<number>(10);
    const started = Date.now();
    const pending = q.wait(60_000);
    setTimeout(() => q.push(7), 5);
    expect(await pending).toBe(true);
    expect(Date.now() - started).toBeLessThan(1_000);
    expect(q.drain()).toEqual([7]);
  });

  test("wait resolves false on keepalive timeout with nothing queued", async () => {
    const q = new EventQueue<number>(10);
    expect(await q.wait(5)).toBe(false);
  });

  test("stop wakes a waiter, resolves false, and ignores later pushes", async () => {
    const q = new EventQueue<number>(10);
    const pending = q.wait(60_000);
    q.stop();
    expect(await pending).toBe(false);
    q.push(1);
    expect(q.size).toBe(0);
    expect(await q.wait(60_000)).toBe(false);
  });
});
