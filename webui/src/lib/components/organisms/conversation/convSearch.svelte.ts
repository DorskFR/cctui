import type { AgentEvent } from '@bindings/AgentEvent';
import type { ConversationHit } from '@bindings/ConversationHit';
import type { ConversationSearchResponse } from '@bindings/ConversationSearchResponse';
import { filters, freeText, parse, type Schema } from '@dorsk/tsumikit';
import { tokenizeQuery } from '$lib/search';
import { buildConversationSearchSchema } from './searchSchema';

export interface ConversationSearchDeps {
	id: () => string;
	/** `GET /sessions/{id}/search`. */
	fetchHits: (id: string, q: string) => Promise<ConversationSearchResponse>;
	/** Pages older history in until `seq` is mounted, then centres it. */
	ensureSeqVisible: (seq: number) => Promise<boolean>;
	onerror?: (e: unknown) => void;
	/** Overridable so tests step the debounce without a timer. */
	debounceMs?: number;
}

/**
 * Find-in-conversation: the query, the server's hit list and the cursor into
 * it.
 *
 * The DOM only holds the paged tail window, so counting `mark.search-hit`
 * cannot give a true total — the server's list is the source of truth and
 * stepping drives {@link ConversationSearchDeps.ensureSeqVisible}, which pages
 * older history in as needed.
 */
export class ConversationSearch {
	#d: ConversationSearchDeps;
	#timer: ReturnType<typeof setTimeout> | null = null;
	#reqId = 0;

	open = $state(false);
	rawQuery = $state('');
	hits = $state<ConversationHit[]>([]);
	index = $state(-1);
	truncated = $state(false);
	loading = $state(false);
	/** Tool ids seen in this session's hits, feeding the `tool:` autocomplete. */
	tools = $state<string[]>([]);

	/** Owned here, not injected: a field initializer cannot read the deps the
	 *  constructor has yet to assign, and the bar wants the same instance. */
	readonly schema: Schema = buildConversationSearchSchema(() => this.tools);

	#ast = $derived(parse(this.rawQuery, this.schema));
	/** Only the free-text part is highlighted; field clauses are not text. */
	terms = $derived(tokenizeQuery(freeText(this.#ast)));
	count = $derived(this.hits.length);
	/** The cursor's `seq`, or null before the first step. */
	currentSeq = $derived(this.hits[this.index]?.seq ?? null);
	/** True once a clause the client cannot evaluate locally is in play, so a
	 *  live event can only be counted by re-asking the server. */
	#fielded = $derived(filters(this.#ast).length > 0);

	constructor(d: ConversationSearchDeps) {
		this.#d = d;
	}

	openBar = (seed?: string): void => {
		this.open = true;
		if (seed !== undefined && seed !== this.rawQuery) {
			this.rawQuery = seed;
			this.schedule();
		}
	};

	/** Esc: clear a filled bar, close an empty one. Returns true when it handled
	 *  the key, so the drawer's own Esc (close) only fires when the bar is idle. */
	escape = (): boolean => {
		if (!this.open) return false;
		if (this.rawQuery) {
			this.setQuery('');
			return true;
		}
		this.close();
		return true;
	};

	close = (): void => {
		this.open = false;
		this.setQuery('');
	};

	setQuery = (raw: string): void => {
		this.rawQuery = raw;
		this.schedule();
	};

	/** Debounced so typing does not fire a request per keystroke. */
	schedule = (): void => {
		if (this.#timer) clearTimeout(this.#timer);
		const raw = this.rawQuery.trim();
		if (!raw) {
			this.#reqId++;
			this.#clearHits();
			return;
		}
		this.#timer = setTimeout(() => void this.run(), this.#d.debounceMs ?? 200);
	};

	run = async (): Promise<void> => {
		const raw = this.rawQuery.trim();
		const id = this.#d.id();
		if (!raw || !id) {
			this.#clearHits();
			return;
		}
		const req = ++this.#reqId;
		this.loading = true;
		try {
			const res = await this.#d.fetchHits(id, raw);
			if (req !== this.#reqId) return;
			const keep = this.currentSeq;
			this.hits = res.hits;
			this.truncated = res.truncated;
			this.tools = res.tools;
			// Re-anchor on the same message when it survived the new query, so a
			// refined query does not throw the user back to the top.
			const at = keep === null ? -1 : res.hits.findIndex((h) => h.seq === keep);
			this.index = at;
		} catch (e) {
			if (req === this.#reqId) this.#d.onerror?.(e);
		} finally {
			if (req === this.#reqId) this.loading = false;
		}
	};

	/**
	 * A live event, counted without a round-trip when the query is pure free
	 * text. With any field clause in play only the server can decide, so the
	 * count catches up on {@link turnEnded}.
	 */
	appendLive = (ev: AgentEvent): void => {
		if (!this.open || this.hits.length === 0 || this.#fielded) return;
		const seq = ev.seq;
		if (typeof seq !== 'number' || this.hits.some((h) => h.seq === seq)) return;
		const text = eventSearchText(ev);
		const terms = this.terms;
		if (terms.length === 0) return;
		const hay = text.toLowerCase();
		if (!terms.every((t) => hay.includes(t.toLowerCase()))) return;
		this.hits = [
			...this.hits,
			{ seq, ts: Number(ev.ts), role: '', tool: null, snippet: text.slice(0, 200) }
		];
	};

	/** Re-ask the server once a turn settles: the cheap local match above cannot
	 *  evaluate `role:`/`tool:`/`pinned:`. */
	turnEnded = (): void => {
		if (this.open && this.rawQuery.trim() && this.#fielded) void this.run();
	};

	next = async (): Promise<void> => {
		if (this.hits.length === 0) return;
		await this.#goto(this.index < 0 ? 0 : (this.index + 1) % this.hits.length);
	};

	prev = async (): Promise<void> => {
		if (this.hits.length === 0) return;
		await this.#goto(this.index <= 0 ? this.hits.length - 1 : this.index - 1);
	};

	reset = (): void => {
		if (this.#timer) clearTimeout(this.#timer);
		this.#timer = null;
		this.#reqId++;
		this.open = false;
		this.rawQuery = '';
		this.#clearHits();
	};

	#clearHits(): void {
		this.hits = [];
		this.index = -1;
		this.truncated = false;
		this.loading = false;
	}

	async #goto(i: number): Promise<void> {
		const hit = this.hits[i];
		if (!hit) return;
		this.index = i;
		await this.#d.ensureSeqVisible(hit.seq);
	}
}

/** The client-side twin of the `stream_events.search_text` generated column
 *  (payload `text` / `content` / `tool` / `input`), so a live match agrees with
 *  what the server would have matched. */
export function eventSearchText(ev: AgentEvent): string {
	const rec = ev as unknown as Record<string, unknown>;
	const parts: string[] = [];
	for (const k of ['content', 'text', 'output_summary', 'tool', 'detail']) {
		const v = rec[k];
		if (typeof v === 'string') parts.push(v);
	}
	const input = rec.input;
	if (input !== undefined && input !== null) {
		parts.push(typeof input === 'string' ? input : JSON.stringify(input));
	}
	return parts.join(' ');
}
