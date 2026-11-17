import { describe, expect, it } from "vitest";
import { bucketOf, groupInboxByLane, type InboxEvent } from "./inbox";

let seq = 0;
function ev(
  lane: string,
  status: InboxEvent["status"],
  minute: number,
  extra: Partial<InboxEvent> = {},
): InboxEvent {
  seq += 1;
  return {
    id: `e${seq}`,
    lane_id: `lane-${lane}`,
    lane_key: lane,
    source: "n8n",
    source_id: null,
    body: `body ${seq}`,
    payload: null,
    status,
    not_before: new Date(Date.UTC(2026, 10, 16, 9, minute)).toISOString(),
    expires_at: null,
    attempts: 0,
    last_error: null,
    batch_id: null,
    delivered_session_id: null,
    delivered_as: null,
    created_at: new Date(Date.UTC(2026, 10, 16, 9, minute)).toISOString(),
    delivered_at: null,
    ...extra,
  };
}

describe("groupInboxByLane", () => {
  it("splits each lane into pending, dead, delivered and cancelled", () => {
    const events = [
      ev("session:a", "delivered", 3, { delivered_session_id: "a" }),
      ev("session:a", "claimed", 5),
      ev("session:a", "pending", 4),
      ev("session:a", "cancelled", 1),
    ];
    const [lane] = groupInboxByLane(events);
    expect(lane.sessionId).toBe("a");
    expect(lane.buckets.pending.map((e) => e.status)).toEqual(["pending", "claimed"]);
    expect(lane.buckets.delivered[0].delivered_session_id).toBe("a");
    expect(lane.buckets.cancelled).toHaveLength(1);
    expect(lane.buckets.dead).toHaveLength(0);
  });

  it("orders lanes with dead events first, then by newest event", () => {
    const lanes = groupInboxByLane([
      ev("session:old", "pending", 1),
      ev("session:new", "delivered", 9),
      ev("preset:p:topic", "dead", 0),
    ]);
    expect(lanes.map((l) => l.key)).toEqual(["preset:p:topic", "session:new", "session:old"]);
    expect(lanes[0].sessionId).toBeNull();
  });

  it("keeps events of one lane together however the list interleaves them", () => {
    const lanes = groupInboxByLane([
      ev("session:a", "pending", 2),
      ev("session:b", "pending", 1),
      ev("session:a", "pending", 1),
    ]);
    expect(lanes).toHaveLength(2);
    expect(lanes[0].buckets.pending.map((e) => e.created_at)).toEqual(
      [...lanes[0].buckets.pending.map((e) => e.created_at)].sort(),
    );
  });
});

describe("bucketOf", () => {
  it("shows a claimed event as pending", () => {
    expect(bucketOf("claimed")).toBe("pending");
    expect(bucketOf("dead")).toBe("dead");
  });
});
