// The server counts `limit` against renderable events and fills the page from
// further back when a stored row has nothing to show, so a page shorter than
// the limit is the head of the transcript.
import type { AgentEvent } from "@bindings/AgentEvent";
import { CONVERSATION_FETCH_LIMIT, endpoints } from "$lib/queries";

export const FOCUS_CONTEXT = 40;

export interface EarlierPagesOpts {
  id: () => string;
  historyLength: () => number;
  events: () => AgentEvent[];
  fetch?: (
    id: string,
    q: { limit: number; before: number },
  ) => Promise<AgentEvent[]>;
}

export class EarlierPages {
  #o: EarlierPagesOpts;
  #fetch: NonNullable<EarlierPagesOpts["fetch"]>;
  #focusFetched: string | null = null;
  pages = $state<AgentEvent[]>([]);
  exhausted = $state(false);
  fetching = $state(false);
  canFetch = $derived.by(
    () =>
      !this.exhausted &&
      (this.#o.historyLength() >= CONVERSATION_FETCH_LIMIT ||
        this.pages.length > 0),
  );

  constructor(o: EarlierPagesOpts) {
    this.#o = o;
    this.#fetch = o.fetch ?? ((id, q) => endpoints.conversation(id, q));
  }

  reset(): void {
    this.pages = [];
    this.exhausted = false;
  }

  fetchEarlier = async (): Promise<void> => {
    if (this.fetching || this.exhausted) return;
    const oldest = this.#o.events().find((e) => typeof e.seq === "number")?.seq;
    if (oldest == null) return;
    const sid = this.#o.id();
    this.fetching = true;
    try {
      const page = await this.#fetch(sid, {
        limit: CONVERSATION_FETCH_LIMIT,
        before: oldest,
      });
      if (sid !== this.#o.id()) return;
      if (page.length < CONVERSATION_FETCH_LIMIT) this.exhausted = true;
      this.pages = [...page, ...this.pages];
    } finally {
      this.fetching = false;
    }
  };

  /** Open centred on a search hit: one extra window prepended here, once per
   *  session+seq. The cached tail query is left alone so live events, dedup
   *  and jump-to-bottom keep working; on a session longer than both windows
   *  the two are not contiguous and the jump-to-bottom pill is the bridge. */
  async focus(sid: string, seq: number): Promise<void> {
    const token = `${sid}|${seq}`;
    if (this.#focusFetched === token) return;
    this.#focusFetched = token;
    const win = await this.#fetch(sid, {
      limit: FOCUS_CONTEXT * 2,
      before: seq + FOCUS_CONTEXT,
    });
    if (sid !== this.#o.id()) return;
    // `before` is an absolute cursor, not a page number: a short window
    // really is the head of the transcript.
    if (win.length < FOCUS_CONTEXT * 2) this.exhausted = true;
    this.pages = [...win, ...this.pages];
  }
}
