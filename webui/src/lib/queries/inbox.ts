import { createQuery, useQueryClient } from "@tanstack/svelte-query";
import { endpoints } from "./endpoints";

/** Mirrors the server's `inbox::store::Event`, which has no generated binding. */
export interface InboxEvent {
  id: string;
  lane_id: string;
  lane_key: string;
  source: string;
  source_id: string | null;
  body: string;
  payload: Record<string, unknown> | null;
  status: "pending" | "claimed" | "delivered" | "cancelled" | "dead";
  not_before: string;
  expires_at: string | null;
  attempts: number;
  last_error: string | null;
  batch_id: string | null;
  delivered_session_id: string | null;
  delivered_as: string | null;
  created_at: string;
  delivered_at: string | null;
}

export type InboxBucket = "pending" | "dead" | "delivered" | "cancelled";

export const INBOX_BUCKETS: readonly InboxBucket[] = [
  "pending",
  "dead",
  "delivered",
  "cancelled",
];

export interface InboxLane {
  id: string;
  key: string;
  sessionId: string | null;
  buckets: Record<InboxBucket, InboxEvent[]>;
}

const SESSION_LANE = "session:";

export function bucketOf(status: InboxEvent["status"]): InboxBucket {
  return status === "claimed" ? "pending" : status;
}

export function groupInboxByLane(events: InboxEvent[]): InboxLane[] {
  const lanes = new Map<string, InboxLane & { newest: number }>();
  for (const e of events) {
    let lane = lanes.get(e.lane_id);
    if (!lane) {
      lane = {
        id: e.lane_id,
        key: e.lane_key,
        sessionId: e.lane_key.startsWith(SESSION_LANE)
          ? e.lane_key.slice(SESSION_LANE.length)
          : null,
        buckets: { pending: [], dead: [], delivered: [], cancelled: [] },
        newest: 0,
      };
      lanes.set(e.lane_id, lane);
    }
    lane.buckets[bucketOf(e.status)].push(e);
    lane.newest = Math.max(lane.newest, Date.parse(e.created_at));
  }
  const at = (e: InboxEvent) => Date.parse(e.created_at);
  return [...lanes.values()]
    .map(({ newest, ...lane }) => {
      for (const b of INBOX_BUCKETS) lane.buckets[b].sort((x, y) => at(x) - at(y));
      return { lane, newest };
    })
    .sort(
      (a, b) =>
        Number(b.lane.buckets.dead.length > 0) -
          Number(a.lane.buckets.dead.length > 0) || b.newest - a.newest,
    )
    .map(({ lane }) => lane);
}

const key = ["inbox", "events"] as const;

export const useInboxEvents = () =>
  createQuery(() => ({
    queryKey: key,
    queryFn: endpoints.inboxEvents,
    refetchInterval: 15_000,
  }));

function useInboxAction(run: (id: string) => Promise<void>) {
  const qc = useQueryClient();
  return async (id: string) => {
    try {
      await run(id);
    } finally {
      await qc.invalidateQueries({ queryKey: key });
    }
  };
}

export const useCancelInboxEvent = () =>
  useInboxAction(endpoints.cancelInboxEvent);

export const useRetryInboxEvent = () =>
  useInboxAction(endpoints.retryInboxEvent);
