import type { AgentEvent } from '@bindings/AgentEvent';
import type { SessionListItem } from '@bindings/SessionListItem';
import type { SessionSuperseded } from '#lib/ws/frames.ts';
import type { ThreadEntry as PageEntry } from '@bindings/ThreadEntry';
import type { ThreadSession } from '@bindings/ThreadSession';
import type { FollowupReason } from '@bindings/FollowupReason';
import { previousSessionId } from '#lib/followup.ts';
import { USER_PREFIX } from '#lib/ws/frames.ts';
import { splitInjected } from './format';
import type { FollowupMark, Line } from './types';

export type { PageEntry };

export const THREAD_START_KEY = 'followup:start';
export const PENDING_MARK_KEY = 'followup:pending';

const REASONS: readonly FollowupReason[] = ['cold', 'manual', 'auto', 'inbox'];

export function inFollowupLine(
	s: Pick<SessionListItem, 'parent_id' | 'metadata' | 'next_session_id'>
): boolean {
	return previousSessionId(s) !== null || !!s.next_session_id;
}

/** `tree` is depth first with siblings oldest first: the last child seen is
 *  the newest follow-up. */
export function linePath(tree: readonly ThreadSession[], opened: string): string[] {
	const byId = new Map(tree.map((t) => [t.id, t]));
	if (!byId.has(opened)) return [opened];
	const up: string[] = [];
	const seen = new Set<string>();
	for (let id: string | null | undefined = opened; id && byId.has(id) && !seen.has(id); ) {
		seen.add(id);
		up.unshift(id);
		id = byId.get(id)?.parent_id;
	}
	const newestChild = new Map<string, string>();
	for (const t of tree) if (t.parent_id && byId.has(t.parent_id)) newestChild.set(t.parent_id, t.id);
	for (let next = newestChild.get(opened); next && !seen.has(next); next = newestChild.get(next)) {
		seen.add(next);
		up.push(next);
	}
	return up;
}

export function markOfSession(s: SessionListItem, generation: number): FollowupMark | null {
	const from = previousSessionId(s);
	const at = s.registered_at;
	if (!from || !at) return null;
	const meta = (s.metadata as Record<string, unknown> | null) ?? {};
	const reason = meta.followup_reason as FollowupReason;
	const instruction = meta.followup_instruction;
	return {
		generation,
		from,
		to: s.id,
		at,
		reason: REASONS.includes(reason) ? reason : null,
		instruction: typeof instruction === 'string' && instruction.trim() ? instruction : null
	};
}

export function markOfEvent(ev: SessionSuperseded, generation: number): FollowupMark {
	return {
		generation,
		from: ev.from,
		to: ev.to,
		at: ev.at,
		reason: ev.reason,
		instruction: ev.instruction.trim() ? ev.instruction : null
	};
}

export function markOfEntry(e: Extract<PageEntry, { entry: 'followup_marker' }>): FollowupMark {
	return {
		generation: e.generation,
		from: e.from,
		to: e.to,
		at: e.at,
		reason: e.reason ?? null,
		instruction: e.instruction?.trim() ? e.instruction : null
	};
}

export function markerEntry(mark: FollowupMark): PageEntry {
	return {
		entry: 'followup_marker',
		generation: mark.generation,
		from: mark.from,
		to: mark.to ?? '',
		at: mark.at,
		reason: mark.reason,
		instruction: mark.instruction
	};
}

export function eventEntries(sessionId: string, generation: number, events: AgentEvent[]): PageEntry[] {
	return events.map((event) => ({ entry: 'event', session_id: sessionId, generation, event }));
}

function eventOf(e: Extract<PageEntry, { entry: 'event' }>): AgentEvent {
	return e.event as unknown as AgentEvent;
}

export function entryKey(e: PageEntry): string {
	if (e.entry === 'followup_marker') return `${e.generation}:m`;
	if (e.entry === 'unreadable') return `${e.generation}:u`;
	const ev = eventOf(e);
	const at = ev.seq ?? `t${ev.ts}`;
	return `${e.generation}:${at}:${e.injected ?? ''}`;
}

export function prependEntries(older: readonly PageEntry[], current: readonly PageEntry[]): PageEntry[] {
	const have = new Set(current.map(entryKey));
	return [...older.filter((e) => !have.has(entryKey(e))), ...current];
}

function userBody(ev: AgentEvent): string | null {
	if (ev.type === 'reply') return ev.content;
	if (ev.type === 'text' && ev.content.startsWith(USER_PREFIX))
		return ev.content.slice(USER_PREFIX.length).trimStart();
	return null;
}

function injectedFirstTurn(body: string): { prose: string } | null {
	const { blocks, prose } = splitInjected(body);
	return blocks.some((b) => b.kind === 'previous-conversation') ? { prose: prose.trim() } : null;
}

/** The marker's instruction stands in for the first turn's prose. */
export function stripFirstTurn(events: AgentEvent[], instruction: string | null): AgentEvent[] {
	const at = events.findIndex((e) => userBody(e) !== null);
	if (at < 0) return events;
	const first = events[at];
	const injected = injectedFirstTurn(userBody(first) ?? '');
	if (!injected) return events;
	const rest = [...events.slice(0, at), ...events.slice(at + 1)];
	if (instruction || !injected.prose) return rest;
	const prose: AgentEvent =
		first.type === 'reply'
			? { ...first, content: injected.prose }
			: { ...(first as Extract<AgentEvent, { type: 'text' }>), content: `${USER_PREFIX} ${injected.prose}` };
	return [...events.slice(0, at), prose, ...events.slice(at + 1)];
}

export function instructionEvent(mark: FollowupMark): AgentEvent | null {
	if (!mark.instruction) return null;
	return {
		type: 'text',
		content: `${USER_PREFIX} ${mark.instruction}`,
		meta: false,
		ts: Date.parse(mark.at)
	};
}

export function generationEvents(events: AgentEvent[], mark: FollowupMark | null): AgentEvent[] {
	if (!mark) return stripFirstTurn(events, null);
	const stripped = stripFirstTurn(events, mark.instruction);
	const lead = instructionEvent(mark);
	return lead ? [lead, ...stripped] : stripped;
}

export interface ThreadSegment {
	sessionId: string;
	generation: number;
	mark: FollowupMark | null;
	unreadable: boolean;
	events: AgentEvent[];
}

export function segmentsOf(entries: readonly PageEntry[]): ThreadSegment[] {
	const marks = new Map<number, FollowupMark>();
	const injectedSeqs = new Map<number, Set<number>>();
	for (const e of entries) {
		if (e.entry === 'followup_marker') marks.set(e.generation, markOfEntry(e));
		if (e.entry === 'event' && e.injected) {
			const seq = eventOf(e).seq;
			if (seq == null) continue;
			const set = injectedSeqs.get(e.generation) ?? new Set<number>();
			set.add(seq);
			injectedSeqs.set(e.generation, set);
		}
	}
	const out: ThreadSegment[] = [];
	const segment = (generation: number, sessionId: string): ThreadSegment => {
		const last = out.at(-1);
		if (last && last.generation === generation) return last;
		const seg: ThreadSegment = {
			sessionId,
			generation,
			mark: null,
			unreadable: false,
			events: []
		};
		out.push(seg);
		return seg;
	};
	for (const e of entries) {
		if (e.entry === 'followup_marker') {
			segment(e.generation, e.to).mark = marks.get(e.generation) ?? null;
			continue;
		}
		if (e.entry === 'unreadable') {
			segment(e.generation, e.session_id).unreadable = true;
			continue;
		}
		if (e.injected) continue;
		const ev = eventOf(e);
		const firstTurn = ev.seq != null && injectedSeqs.get(e.generation)?.has(ev.seq);
		if (firstTurn && marks.get(e.generation)?.instruction) continue;
		segment(e.generation, e.session_id).events.push(ev);
	}
	for (const seg of out) {
		if (seg.generation >= 2) seg.events = generationEvents(seg.events, seg.mark);
	}
	return out;
}

export function markerLine(mark: FollowupMark): Line {
	return {
		role: 'system',
		ts: Date.parse(mark.at),
		key: mark.pending || !mark.to ? PENDING_MARK_KEY : `followup:${mark.to}`,
		followup: mark
	};
}

/** Lines of a later generation carry its session: their keys stay unique
 *  across generations and survive the head moving on. */
export function ownLines(lines: Line[], sessionId: string, generation: number): Line[] {
	if (generation < 2) return lines;
	return lines.map((ln) => ({ ...ln, key: `${sessionId}:${ln.key ?? ''}`, sessionId }));
}

export function threadLines(
	segments: readonly ThreadSegment[],
	build: (seg: ThreadSegment) => Line[]
): Line[] {
	const out: Line[] = [];
	for (const seg of segments) {
		if (seg.mark) out.push(markerLine(seg.mark));
		if (seg.unreadable) {
			out.push({
				role: 'system',
				ts: 0,
				key: `followup:unreadable:${seg.sessionId}`,
				unreadable: true,
				sessionId: seg.sessionId
			});
		}
		out.push(...ownLines(build(seg), seg.sessionId, seg.generation));
	}
	return out;
}

/** The row a link to `sessionId` lands on: its marker, or the top of the
 *  thread for the root. Null while that part of the thread is not loaded. */
export function anchorFor(lines: readonly Line[], sessionId: string, rootId: string | null): string | null {
	if (sessionId === rootId) return lines.length ? THREAD_START_KEY : null;
	const hit = lines.find((l) => l.followup && l.followup.to === sessionId);
	return hit?.key ?? null;
}
