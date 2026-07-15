import { describe, it, expect } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { orderEvents } from './format';

// CCT-481: the merged history+live event list is ordered by the server's
// monotonic per-session insert `seq` (`stream_events.id`), not receive-time
// `ts`. `seq` reflects true causal order, so a late-flushed AskUserQuestion —
// whose card + preamble carry a `ts` at/after the user's answer — still renders
// above its answer, which a `ts`-only sort inverted (CCT-475).

const preamble = (ts: number, seq: number | null): AgentEvent => ({
	type: 'text',
	content: 'I need to know which DB to use.',
	meta: false,
	ts,
	message_id: null,
	usage: null,
	seq
});
const askCard = (ts: number, seq: number | null): AgentEvent => ({
	type: 'tool_call',
	tool: 'AskUserQuestion',
	input: {},
	ts,
	seq
});
const answer = (ts: number, seq: number | null): AgentEvent => ({
	type: 'text',
	content: '▷ User: Postgres',
	meta: false,
	ts,
	message_id: null,
	usage: null,
	seq
});

const seqs = (es: AgentEvent[]) => es.map((e) => e.seq);
const contents = (es: AgentEvent[]) =>
	es.map((e) => {
		if (e.type === 'text') return e.content;
		if (e.type === 'tool_call' || e.type === 'tool_result') return e.tool;
		return e.type;
	});

describe('orderEvents (CCT-481)', () => {
	it('orders by seq when ts ties', () => {
		// All three share ts=100; only seq distinguishes causal order. A shuffled
		// input (answer first) must still come out preamble → card → answer.
		const ordered = orderEvents([answer(100, 3), preamble(100, 1), askCard(100, 2)]);
		expect(seqs(ordered)).toEqual([1, 2, 3]);
		expect(contents(ordered)).toEqual(['I need to know which DB to use.', 'AskUserQuestion', '▷ User: Postgres']);
	});

	it('orders by seq even when ts is inverted (late flush)', () => {
		// The late-flushed preamble+card carry a LATER ts than the answer, yet a
		// LOWER seq — ordering by seq restores the ask above its answer.
		const ordered = orderEvents([answer(100, 1), preamble(200, 2), askCard(210, 3)]);
		// seq order is answer(1), preamble(2), card(3) here — the answer genuinely
		// preceded this ask; seq is the source of truth, not the inverted ts.
		expect(seqs(ordered)).toEqual([1, 2, 3]);

		// And the true late-flush case: card+preamble inserted BEFORE the answer
		// (lower seq) but stamped with a later ts — seq keeps them on top.
		const lateFlush = orderEvents([answer(100, 3), preamble(150, 1), askCard(160, 2)]);
		expect(seqs(lateFlush)).toEqual([1, 2, 3]);
		expect(contents(lateFlush)[2]).toBe('▷ User: Postgres');
	});

	it('falls back to ts when seq is absent (legacy payloads)', () => {
		const ordered = orderEvents([answer(300, null), preamble(100, null), askCard(200, null)]);
		expect(ordered.map((e) => e.ts)).toEqual([100, 200, 300]);
	});
});
