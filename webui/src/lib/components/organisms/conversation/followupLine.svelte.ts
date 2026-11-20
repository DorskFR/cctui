import type { AgentEvent } from '@bindings/AgentEvent';
import type { FollowupReason } from '@bindings/FollowupReason';
import type { FollowupResponse } from '@bindings/FollowupResponse';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionThreadResponse } from '@bindings/SessionThreadResponse';
import type { ThreadConversationPage } from '@bindings/ThreadConversationPage';
import type { FollowupFailed, SessionSuperseded } from '#lib/ws/frames.ts';
import {
	eventEntries,
	inFollowupLine,
	linePath,
	markOfEntry,
	markOfEvent,
	markOfSession,
	markerEntry,
	prependEntries,
	segmentsOf,
	type PageEntry
} from './threadView';
import type { FollowupMark } from './types';

export interface FollowupLineDeps {
	thread: (id: string, opts?: { limit?: number; cursor?: string }) => Promise<ThreadConversationPage>;
	tree: (id: string) => Promise<SessionThreadResponse>;
	session: (id: string) => Promise<SessionListItem>;
}

export interface HeadSnapshot {
	session: SessionListItem;
	events: AgentEvent[];
	complete: boolean;
}

/** Stands in for the new head until its row is fetched, so the composer
 *  targets it at once. */
export function provisionalHead(source: SessionListItem, ev: SessionSuperseded): SessionListItem {
	const meta = (source.metadata as Record<string, unknown> | null) ?? {};
	return {
		...source,
		id: ev.to,
		status: 'active',
		parent_id: ev.from,
		next_session_id: null,
		registered_at: ev.at,
		last_activity_at: ev.at,
		latest_outcome: null,
		metadata: {
			...meta,
			relation: 'followup',
			followup_reason: ev.reason,
			followup_instruction: ev.instruction
		}
	} as SessionListItem;
}

export class FollowupLine {
	#d: FollowupLineDeps;
	#epoch = 0;
	#anchor: string | null = null;
	#pendingId: string | null = null;
	#swapped = new Set<string>();
	#inflight: Promise<void> | null = null;
	/** Settles once `open` has placed the head and fetched the first page. */
	ready: Promise<void> = Promise.resolve();
	/** Not reactive: read untracked. */
	members = new Set<string>();
	rootId = $state<string | null>(null);
	headId = $state<string | null>(null);
	head = $state<SessionListItem | null>(null);
	headGen = $state(1);
	headMark = $state<FollowupMark | null>(null);
	entries = $state.raw<PageEntry[]>([]);
	cursor = $state<string | null>(null);
	fetching = $state(false);
	pending = $state<FollowupMark | null>(null);
	/** The session the pane was opened on, when the head is a later one. */
	target = $state<string | null>(null);
	/** The head started in this pane: it has no history to wait for. */
	fresh = $state(false);

	active = $derived(this.headGen >= 2 || this.entries.length > 0);
	segments = $derived(segmentsOf(this.entries));
	canFetchOlder = $derived(this.cursor !== null);

	constructor(deps: FollowupLineDeps) {
		this.#d = deps;
	}

	open(s: SessionListItem): Promise<void> {
		const epoch = ++this.#epoch;
		this.members = new Set([s.id]);
		this.rootId = null;
		this.headId = s.id;
		this.head = null;
		this.headGen = 1;
		this.headMark = null;
		this.entries = [];
		this.cursor = null;
		this.fetching = false;
		this.pending = null;
		this.target = null;
		this.fresh = false;
		this.#anchor = null;
		this.#pendingId = null;
		this.#inflight = null;
		this.ready = inFollowupLine(s) ? this.#load(s, epoch) : Promise.resolve();
		return this.ready;
	}

	async #load(s: SessionListItem, epoch: number): Promise<void> {
		try {
			const { sessions } = await this.#d.tree(s.id);
			if (epoch !== this.#epoch) return;
			const path = linePath(sessions, s.id);
			for (const id of path) this.members.add(id);
			const headId = path.at(-1) ?? s.id;
			const head = headId === s.id ? s : await this.#d.session(headId);
			if (epoch !== this.#epoch) return;
			const probe = await this.#d.thread(headId, { limit: 1 });
			if (epoch !== this.#epoch) return;
			for (const g of probe.generations) this.members.add(g.session_id);
			const generation = probe.generations.find((g) => g.session_id === headId)?.generation ?? 1;
			this.rootId = probe.generations[0]?.session_id ?? probe.thread_root_id;
			this.headId = headId;
			this.head = headId === s.id ? null : head;
			this.headGen = generation;
			this.headMark = markOfSession(head, generation);
			this.target = headId === s.id ? null : s.id;
			if (generation < 2) return;
			this.#anchor = headId;
			// Everything before the head's first event, its marker included.
			this.cursor = `${generation}:0`;
			await this.fetchOlder();
		} catch {
			// The session still renders on its own.
		}
	}

	fetchOlder = (): Promise<void> => {
		if (this.#inflight) return this.#inflight;
		const cursor = this.cursor;
		const anchor = this.#anchor;
		if (!cursor || !anchor) return Promise.resolve();
		const run = this.#fetchPage(anchor, cursor).finally(() => {
			if (this.#inflight === run) this.#inflight = null;
		});
		this.#inflight = run;
		return run;
	};

	async #fetchPage(anchor: string, cursor: string): Promise<void> {
		const epoch = this.#epoch;
		this.fetching = true;
		try {
			const page = await this.#d.thread(anchor, { cursor });
			if (epoch !== this.#epoch || cursor !== this.cursor) return;
			const own = page.entries.filter((e) => e.generation < this.headGen);
			const mark = page.entries.find(
				(e) => e.entry === 'followup_marker' && e.generation === this.headGen
			);
			if (mark?.entry === 'followup_marker') this.headMark = markOfEntry(mark);
			this.entries = prependEntries(own, this.entries);
			this.cursor = page.next_cursor ?? null;
		} finally {
			if (epoch === this.#epoch) this.fetching = false;
		}
	}

	begin(res: FollowupResponse, instruction: string, reason: FollowupReason): void {
		if (this.#swapped.has(res.followup_id) || res.from !== this.headId) return;
		this.#pendingId = res.followup_id;
		this.pending = {
			generation: this.headGen + 1,
			from: res.from,
			to: null,
			at: new Date().toISOString(),
			reason,
			instruction: instruction.trim() || null,
			pending: true
		};
	}

	fail(ev: FollowupFailed): boolean {
		if (ev.from !== this.headId) return false;
		if (this.#pendingId !== null && this.#pendingId !== ev.followup_id) return false;
		this.pending = null;
		this.#pendingId = null;
		return true;
	}

	supersede(ev: SessionSuperseded, cur: HeadSnapshot): boolean {
		if (ev.from !== this.headId || this.#swapped.has(ev.followup_id)) return false;
		this.#swapped.add(ev.followup_id);
		const generation = this.headGen;
		const frozen = eventEntries(ev.from, generation, cur.events);
		if (cur.complete) {
			const lead = this.headMark && generation >= 2 ? [markerEntry(this.headMark)] : [];
			this.entries = [...this.entries, ...lead, ...frozen];
		} else {
			// Its older events and marker come back through the cursor.
			const oldest = cur.events.find((e) => typeof e.seq === 'number')?.seq;
			this.entries = frozen;
			this.cursor = oldest == null ? null : `${generation}:${oldest}`;
		}
		this.#anchor = ev.from;
		this.rootId ??= ev.from;
		this.members.add(ev.from);
		this.members.add(ev.to);
		this.headMark = markOfEvent(ev, generation + 1);
		this.headGen = generation + 1;
		this.headId = ev.to;
		this.head = provisionalHead(cur.session, ev);
		this.fresh = true;
		this.pending = null;
		this.#pendingId = null;
		this.target = null;
		const epoch = this.#epoch;
		void this.#d.session(ev.to).then(
			(row) => {
				if (epoch === this.#epoch && this.headId === row.id) this.head = row;
			},
			() => {}
		);
		return true;
	}
}
