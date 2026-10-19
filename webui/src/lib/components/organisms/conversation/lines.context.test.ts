import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { allFilter, defaultFilter } from './filters';
import { lineMarkdown } from './format';
import { buildLines, type LineBuildCtx } from './lines';
import type { MsgCategory, MsgFilter } from './types';

const ctxOf = (filter: MsgFilter): LineBuildCtx => ({
	visible: (c) => filter[c],
	renderMarkdown: (s) => `<p>${s}</p>`,
	renderCode: (text) => `<code>${text}</code>`,
	prettyJson: true,
	prettyDiff: true
});
const ctx = (overrides: Partial<Record<MsgCategory, boolean>> = {}) =>
	ctxOf({ ...allFilter(true), ...overrides });

const user = (content: string, ts: number, seq: number | null = null): AgentEvent => ({
	type: 'text',
	content: `▷ User: ${content}`,
	meta: false,
	kind: null,
	ts,
	message_id: null,
	usage: null,
	seq
});

const SESSION = '<cctui-session-context>\nsession: lane e\ncwd: /w\n</cctui-session-context>';
const PREVIOUS =
	'<cctui-previous-conversation session="lane d" id="s-prev">\n**User:**\n\nq\n</cctui-previous-conversation>';

describe('injected context segments', () => {
	it('renders a fresh spawn as [context, user]', () => {
		const lines = buildLines([user(`${SESSION}\n\nfix the bug`, 1)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['context', 'user']);
		expect(lines[0].context?.kind).toBe('session-context');
		expect(lines[0].text).toBe('session: lane e\ncwd: /w');
		expect(lines[1].text).toBe('fix the bug');
	});

	it('renders a follow-up as [session, previous conversation, user] with the source session', () => {
		const lines = buildLines([user(`${SESSION}\n\n${PREVIOUS}\n\nContinue from this.`, 1)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['context', 'context', 'user']);
		expect(lines[1].context).toMatchObject({
			kind: 'previous-conversation',
			session: 'lane d',
			sessionId: 's-prev'
		});
		expect(lines[2].text).toBe('Continue from this.');
	});

	it('splits a legacy follow-up across its === separator', () => {
		const legacy =
			'<session-context>\nx\n</session-context>\n\n<previous-conversation session="a" id="s1">\nb\n</previous-conversation>\n\n===\n\nask';
		const lines = buildLines([user(legacy, 1)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['context', 'context', 'user']);
		expect(lines[2].text).toBe('ask');
	});

	it('omits the user line when the turn is only context', () => {
		const lines = buildLines([user(SESSION, 1, 7)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['context']);
		expect(lines[0].seq).toBe(7);
	});

	it('keeps a human quoting a wrapper mid-text as one user line', () => {
		const raw = `look at this:\n${SESSION}`;
		const lines = buildLines([user(raw, 1)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['user']);
		expect(lines[0].text).toBe(raw);
	});

	it('classifies the prose after the blocks, not the raw turn', () => {
		const peer = '<cctui-peer-message from="s1" from-name="lane-a">\nhi\n</cctui-peer-message>';
		const lines = buildLines([user(`${SESSION}\n\n${peer}`, 1)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['context', 'peer']);
		expect(lines[1].peerFrom).toBe('lane-a');
		const sys = buildLines([user(`${SESSION}\n\n<system-reminder>be brief</system-reminder>`, 2)], ctx());
		expect(sys.map((l) => l.role)).toEqual(['context', 'system']);
	});

	it('gives the prose line the seq, so pins and jumps land on the ask', () => {
		const lines = buildLines([user(`${SESSION}\n\nfix the bug`, 1, 42)], ctx());
		expect(lines.map((l) => l.seq)).toEqual([undefined, 42]);
	});

	it('keys the blocks off the prose line, which keeps the turn key', () => {
		const lines = buildLines([user(`${SESSION}\n\n${PREVIOUS}\n\nfix the bug`, 5)], ctx());
		const prose = lines[2].key;
		expect(prose).toBe('5|user|fix the bug');
		expect(lines.map((l) => l.key)).toEqual([`${prose}#ctx0`, `${prose}#ctx1`, prose]);
	});

	it('keeps keys unique when two context-only turns share a timestamp', () => {
		const lines = buildLines(
			[user(SESSION, 1, 1), user('<cctui-session-context>\ny\n</cctui-session-context>', 1, 2)],
			ctx()
		);
		const keys = lines.map((l) => l.key);
		expect(new Set(keys).size).toBe(keys.length);
	});

	it('collapses a duplicate encoding of the same segmented turn', () => {
		const raw = `${SESSION}\n\nfix the bug`;
		const lines = buildLines([user(raw, 1), user(raw, 1)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['context', 'user']);
	});

	it('hides only the context lines when the context filter is off', () => {
		const lines = buildLines([user(`${SESSION}\n\nfix the bug`, 1, 3)], ctx({ context: false }));
		expect(lines.map((l) => l.role)).toEqual(['user']);
		expect(lines[0].text).toBe('fix the bug');
		expect(lines[0].seq).toBe(3);
	});

	it('shows context under the default filter', () => {
		const lines = buildLines([user(`${SESSION}\n\nfix the bug`, 1)], ctxOf(defaultFilter()));
		expect(lines.map((l) => l.role)).toEqual(['context', 'user']);
	});

	it('copies only the prose from the user line and only the block from the context line', () => {
		const lines = buildLines([user(`${SESSION}\n\nfix the bug`, 1)], ctx());
		expect(lineMarkdown(lines[1])).toBe('fix the bug');
		expect(lineMarkdown(lines[0])).toBe('session: lane e\ncwd: /w');
	});
});
