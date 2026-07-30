import type { QueryClient } from "@tanstack/svelte-query";
import { baseUrl, getToken } from "./config";
import { keys } from "./queries";
import type { SseEvent } from "./types";

export type QueryKeyAction =
  | { type: "invalidate"; key: readonly unknown[] }
  | { type: "refetch"; key: readonly unknown[] };

export function sseActions(event: SseEvent): QueryKeyAction[] {
  switch (event.event) {
    case "pr.updated": {
      const { owner, repo, number } = event.data;
      return [
        { type: "invalidate", key: keys.pull(owner, repo, number) },
        { type: "invalidate", key: keys.pullViewed(owner, repo, number) },
        { type: "invalidate", key: keys.reviewThreads(owner, repo, number) },
        { type: "invalidate", key: keys.reviewers(owner, repo, number) },
        { type: "invalidate", key: keys.activityAll(owner, repo, number) },
        { type: "invalidate", key: keys.repoLabelsAll(owner, repo) },
        { type: "invalidate", key: keys.pullsAll() },
      ];
    }
    case "pr.viewed_state.updated": {
      const { owner, repo, number } = event.data;
      return [{ type: "invalidate", key: keys.pullViewed(owner, repo, number) }];
    }
    case "notification.new":
    case "notification.updated":
      return [{ type: "invalidate", key: keys.notificationsAll() }];
    case "sync.status":
      return [{ type: "invalidate", key: keys.status() }];
    default:
      return [];
  }
}

export function applySseEvent(client: QueryClient, event: SseEvent): void {
  for (const action of sseActions(event)) {
    client.invalidateQueries({ queryKey: action.key });
  }
}

export type SseListener = (event: SseEvent) => void;

export interface SseHandle {
  close(): void;
}

export function subscribeSse(client: QueryClient, onEvent?: SseListener): SseHandle {
  const token = getToken();
  const url = new URL(`${baseUrl()}/v1/events`, window.location.origin);
  if (token) url.searchParams.set("access_token", token);

  const source = new EventSource(url.toString());
  const named = [
    "pr.updated",
    "pr.viewed_state.updated",
    "notification.new",
    "notification.updated",
    "sync.status",
  ];

  const handle = (raw: MessageEvent, name: string) => {
    if (!raw.data) return;
    try {
      const data = JSON.parse(raw.data);
      const event = { event: name, data } as SseEvent;
      applySseEvent(client, event);
      onEvent?.(event);
    } catch {
      onEvent?.({ event: name } as unknown as SseEvent);
    }
  };

  for (const name of named) {
    source.addEventListener(name, (e) => handle(e as MessageEvent, name));
  }

  // EventSource auto-reconnects while CONNECTING; CLOSED is terminal (a rejected
  // token or non-200), so stop rather than spin in a permanent error state. A
  // real auth failure re-gates via the 401 path on the next request.
  source.onerror = () => {
    if (source.readyState === EventSource.CLOSED) {
      source.close();
      console.warn("gh-review: event stream closed");
    }
  };

  return { close: () => source.close() };
}
