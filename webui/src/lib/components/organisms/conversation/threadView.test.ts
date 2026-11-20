import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import type { ThreadEntry } from '@bindings/ThreadEntry';
import type { JsonValue } from '@bindings/serde_json/JsonValue';
import type { InjectedFlag } from '@bindings/InjectedFlag';
import { allFilter } from './filters';
import { buildLines, type LineBuildCtx } from './lines';
import {
	THREAD_START_KEY,
	anchorFor,
	generationEvents,
	linePath,
	segmentsOf,
	stripFirstTurn,
	threadLines,
	type ThreadSegment
} from './threadView';
import type { Line } from './types';

const ctx: LineBuildCtx = {
	visible: (c) => allFilter(true)[c],
	renderMarkdown: (s) => `<p>${s}</p>`,
	renderCode: (text) => `<code>${text}</code>`,
	prettyJson: true,
	prettyDiff: true
};
const build = (seg: ThreadSegment): Line[] => buildLines(seg.events, ctx);

const T0 = Date.parse('2026-11-20T10:00:00Z');
const user = (content: string, seq: number, ts = T0 + seq * 1000): AgentEvent => ({
	type: 'text',
	content: `▷ User: ${content}`,
	meta: false,
	ts,
	seq
});
type TextEvent = Extract<AgentEvent, { type: 'text' }>;
const said = (content: string, seq: number, ts = T0 + seq * 1000): TextEvent => ({
	type: 'text',
	content,
	meta: false,
	ts,
	seq
});
const tool = (seq: number): AgentEvent => ({
	type: 'tool_call',
	tool: 'Bash',
	input: { command: `ls ${seq}` },
	ts: T0 + seq * 1000,
	seq
});
const ev = (session_id: string, generation: number, event: AgentEvent): ThreadEntry => ({
	entry: 'event',
	session_id,
	generation,
	event: event as unknown as JsonValue
});
const flagged = (
	session_id: string,
	generation: number,
	injected: InjectedFlag,
	content: string,
	seq: number
): ThreadEntry => ({
	entry: 'event',
	session_id,
	generation,
	injected,
	event: { ...said(content, seq), meta: true } as unknown as JsonValue
});
const marker = (generation: number, from: string, to: string, instruction: string | null): ThreadEntry => ({
	entry: 'followup_marker',
	generation,
	from,
	to,
	at: `2026-11-20T1${generation}:00:00Z`,
	reason: 'cold',
	instruction
});

const BRIEF =
	'<cctui-previous-sessions>\n<cctui-previous-conversation n="1" id="A">\nthe whole brief of A\n</cctui-previous-conversation>\n</cctui-previous-sessions>';
const CONTEXT = '<cctui-session-context>\nsession: B\n</cctui-session-context>';

function line(): ThreadEntry[] {
	return [
		ev('A', 1, user('start', 1)),
		ev('A', 1, tool(2)),
		ev('A', 1, said('done in A', 3)),
		marker(2, 'A', 'B', 'Carry on in B.'),
		flagged('B', 2, 'previous_sessions', BRIEF, 1),
		flagged('B', 2, 'session_context', CONTEXT, 1),
		ev('B', 2, user('Carry on in B.', 1)),
		ev('B', 2, tool(2)),
		ev('B', 2, said('done in B', 3)),
		marker(3, 'B', 'C', 'Carry on in C.'),
		flagged('C', 3, 'previous_sessions', BRIEF, 1),
		ev('C', 3, user('Carry on in C.', 1)),
		ev('C', 3, tool(2))
	];
}

const rendered = () => threadLines(segmentsOf(line()), build);

describe('thread rendering', () => {
	it('markers sit between generations', () => {
		const lines = rendered();
		const markers = lines.flatMap((l, i) => (l.followup ? [i] : []));
		expect(markers).toHaveLength(2);
		expect(lines[markers[0]].followup).toMatchObject({ from: 'A', to: 'B', reason: 'cold' });
		expect(lines[markers[1]].followup).toMatchObject({ from: 'B', to: 'C' });
		expect(lines.slice(0, markers[0]).some((l) => l.text?.includes('done in A'))).toBe(true);
		const between = lines.slice(markers[0] + 1, markers[1]);
		expect(between.some((l) => l.text?.includes('done in B'))).toBe(true);
		expect(between.filter((l) => l.role === 'tool')).toHaveLength(1);
		expect(lines.slice(markers[1] + 1).filter((l) => l.role === 'tool')).toHaveLength(1);
		expect(new Set(lines.map((l) => l.key)).size).toBe(lines.length);
		expect(between.every((l) => l.sessionId === 'B')).toBe(true);
	});

	it('injected first-turn blocks of later generations are hidden', () => {
		const lines = rendered();
		expect(lines.some((l) => l.role === 'context')).toBe(false);
		expect(lines.some((l) => (l.text ?? '').includes('the whole brief of A'))).toBe(false);
		expect(lines.some((l) => (l.text ?? '').includes('session: B'))).toBe(false);

		const frozenHead = [user(`${BRIEF}\n\n${CONTEXT}\n\nCarry on in B.`, 1), said('done in B', 2)];
		expect(stripFirstTurn(frozenHead, 'Carry on in B.')).toEqual([frozenHead[1]]);
		const legacy = stripFirstTurn(frozenHead, null);
		expect(legacy).toHaveLength(2);
		expect((legacy[0] as { content: string }).content).toBe('▷ User: Carry on in B.');
		const later = [said('x', 1), user(`${CONTEXT}\n\nlater attach`, 2)];
		expect(stripFirstTurn(later, 'i')).toBe(later);
	});

	it('the instruction renders as a user message after the marker', () => {
		const lines = rendered();
		for (const to of ['B', 'C']) {
			const at = lines.findIndex((l) => l.followup?.to === to);
			expect(lines[at + 1].role).toBe('user');
			expect(lines[at + 1].text).toContain(`Carry on in ${to}.`);
			expect(lines.filter((l) => l.text?.includes(`Carry on in ${to}.`))).toHaveLength(1);
		}
		const head = generationEvents([user(`${BRIEF}\n\nGo.`, 1), tool(2)], {
			generation: 2,
			from: 'A',
			to: 'B',
			at: '2026-11-20T10:00:00Z',
			reason: 'manual',
			instruction: 'Go.'
		});
		expect(head.map((e) => e.type)).toEqual(['text', 'tool_call']);
		expect((head[0] as { content: string }).content).toBe('▷ User: Go.');
	});

	it('a deep link resolves to its generation marker', () => {
		const lines = rendered();
		expect(anchorFor(lines, 'B', 'A')).toBe('followup:B');
		expect(anchorFor(lines, 'C', 'A')).toBe('followup:C');
		expect(anchorFor(lines, 'A', 'A')).toBe(THREAD_START_KEY);
		expect(anchorFor(lines, 'elsewhere', 'A')).toBeNull();
		expect(anchorFor([], 'A', 'A')).toBeNull();
	});

	it('follows the opened session to the newest follow-up of each generation', () => {
		const tree = [
			{ id: 'A', working_dir: '/w', registered_at: '2026-11-20T10:00:00Z', parent_id: null },
			{ id: 'B', working_dir: '/w', registered_at: '2026-11-20T11:00:00Z', parent_id: 'A' },
			{ id: 'C1', working_dir: '/w', registered_at: '2026-11-20T12:00:00Z', parent_id: 'B' },
			{ id: 'C2', working_dir: '/w', registered_at: '2026-11-20T13:00:00Z', parent_id: 'B' },
			{ id: 'S', working_dir: '/w', registered_at: '2026-11-20T11:30:00Z', parent_id: 'A' }
		];
		expect(linePath(tree, 'B')).toEqual(['A', 'B', 'C2']);
		expect(linePath(tree, 'C1')).toEqual(['A', 'B', 'C1']);
		expect(linePath(tree, 'S')).toEqual(['A', 'S']);
		expect(linePath(tree, 'missing')).toEqual(['missing']);
	});

	it('shows an unreadable generation as a placeholder', () => {
		const lines = threadLines(
			segmentsOf([
				ev('A', 1, user('start', 1)),
				marker(2, 'A', 'B', null),
				{ entry: 'unreadable', session_id: 'B', generation: 2 }
			]),
			build
		);
		expect(lines.map((l) => (l.followup ? 'marker' : l.unreadable ? 'gap' : l.role))).toEqual([
			'user',
			'marker',
			'gap'
		]);
	});
});
