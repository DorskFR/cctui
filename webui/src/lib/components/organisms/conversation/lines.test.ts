import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { allFilter } from './filters';
import { buildLines, type LineBuildCtx } from './lines';
import type { MsgCategory } from './types';

const ctx = (overrides: Partial<Record<MsgCategory, boolean>> = {}): LineBuildCtx => {
	const filter = { ...allFilter(true), ...overrides };
	return {
		visible: (c) => filter[c],
		renderMarkdown: (s) => `<p>${s}</p>`,
		renderCode: (text) => `<code>${text}</code>`,
		prettyJson: true,
		prettyDiff: true
	};
};

const only = (...cats: MsgCategory[]): LineBuildCtx => {
	const filter = allFilter(false);
	for (const c of cats) filter[c] = true;
	return { ...ctx(), visible: (c) => filter[c] };
};

const text = (
	content: string,
	ts: number,
	kind: string | null = null,
	seq: number | null = null
): AgentEvent => ({
	type: 'text',
	content,
	meta: false,
	kind,
	ts,
	message_id: null,
	usage: null,
	seq
});

const summary = (
	detail: string,
	ts: number,
	opts: { needsAction?: boolean; category?: string | null } = {}
): AgentEvent => ({
	type: 'turn_summary',
	detail,
	status_category: opts.category ?? null,
	needs_action: opts.needsAction ?? false,
	ts,
	seq: null
});

const roles = (es: AgentEvent[], c = ctx()) => buildLines(es, c).map((l) => l.role);

const toolCall = (tool: string, ts: number, kind: string | null = null): AgentEvent => ({
	type: 'tool_call',
	tool,
	input: {},
	kind,
	ts,
	seq: null
});

const toolResult = (
	ts: number,
	opts: { kind?: string | null; error?: boolean; output?: string } = {}
): AgentEvent => ({
	type: 'tool_result',
	tool: 'Bash',
	output_summary: opts.output ?? 'ok',
	kind: opts.kind ?? null,
	error: opts.error ?? false,
	ts,
	seq: null
});

describe('per-category visibility', () => {
	const events: AgentEvent[] = [
		text('▷ User: go', 1),
		text('▷ User: <system-reminder>be brief</system-reminder>', 2),
		text('thought', 3, 'thinking'),
		text('[redacted thinking]', 4, 'redacted_thinking'),
		text('answer', 5),
		text('[image attachment]', 6, 'attachment'),
		text('· permission mode: plan', 7, 'system_marker'),
		toolCall('Bash', 8),
		toolCall('mcp__pg__query', 9),
		toolResult(10),
		{ type: 'compact_summary', content: 'so far…', ts: 11, seq: null },
		{ type: 'context_reset', ts: 12, seq: null },
		summary('wrapped up', 13),
		toolCall('web_search', 14, 'server_tool_use'),
		toolResult(15, { kind: 'server_tool_result', output: 'search hits' }),
		toolResult(16, { error: true, output: 'boom' })
	];

	const cases: [MsgCategory, string, number][] = [
		['user', 'user', 1],
		['system', 'system', 2],
		['thinking', 'thinking', 3],
		['redacted', 'thinking', 4],
		['assistant', 'assistant', 5],
		['attachment', 'assistant', 6],
		['marker', 'marker', 7],
		['tool', 'tool', 8],
		['mcp', 'tool', 9],
		['result', 'result', 10],
		['compact', 'compact', 11],
		['reset', 'reset', 12],
		['summary', 'summary', 13],
		['server_tool', 'tool', 14],
		['server_result', 'result', 15],
		['error', 'result', 16]
	];

	it.each(cases)('renders only its own line when %s is the sole category', (cat, role, ts) => {
		const shown = buildLines(events, only(cat));
		expect(shown.map((l) => [l.role, l.ts])).toEqual([[role, ts]]);
	});

	it.each(cases)('drops its own line, and only that one, when %s is off', (cat, _role, ts) => {
		const kept = buildLines(events, ctx({ [cat]: false }));
		expect(kept.map((l) => l.ts)).not.toContain(ts);
		for (const [, , otherTs] of cases) {
			// A summary with no assistant bubble left to hang on is a line of its
			// own; once one exists again it becomes that bubble's footer.
			if (otherTs === ts || otherTs === 13) continue;
			expect(kept.map((l) => l.ts)).toContain(otherTs);
		}
	});

	it('renders every category when nothing is filtered', () => {
		expect(roles(events)).toEqual([
			'user',
			'system',
			'thinking',
			'thinking',
			'assistant',
			'assistant',
			'marker',
			'tool',
			'tool',
			'result',
			'compact',
			'reset',
			'summary',
			'tool',
			'result',
			'result'
		]);
	});

	it('classes an errored server result under error, not server_result', () => {
		const events = [toolResult(1, { kind: 'server_tool_result', error: true })];
		expect(roles(events, only('server_result'))).toEqual([]);
		expect(roles(events, only('error'))).toEqual(['result']);
	});
});

describe('thinking lines', () => {
	it('maps kind "thinking" to its own role, not assistant', () => {
		const lines = buildLines([text('weighing options', 1, 'thinking')], ctx());
		expect(lines).toHaveLength(1);
		expect(lines[0].role).toBe('thinking');
		expect(lines[0].text).toBe('weighing options');
		expect(lines[0].redacted).toBe(false);
	});

	it('maps kind "redacted_thinking" to a thinking line flagged redacted', () => {
		const lines = buildLines([text('[redacted thinking]', 1, 'redacted_thinking')], ctx());
		expect(lines[0].role).toBe('thinking');
		expect(lines[0].redacted).toBe(true);
	});

	it('leaves plain text (kind null) an ordinary assistant line', () => {
		const lines = buildLines([text('here is the answer', 1)], ctx());
		expect(lines[0].role).toBe('assistant');
		expect(lines[0].redacted).toBeUndefined();
	});

	it('keeps attachments assistant-side and markers on their own role', () => {
		expect(roles([text('a file', 1, 'attachment')])).toEqual(['assistant']);
		expect(roles([text('· agent name: qa', 1, 'system_marker')])).toEqual(['marker']);
	});

	it('filters redacted thinking apart from ordinary thinking', () => {
		const events = [
			text('thought', 1, 'thinking'),
			text('[redacted thinking]', 2, 'redacted_thinking')
		];
		expect(buildLines(events, ctx({ redacted: false })).map((l) => l.text)).toEqual(['thought']);
		expect(buildLines(events, ctx({ thinking: false })).map((l) => l.text)).toEqual([
			'[redacted thinking]'
		]);
	});

	it('is shown by default and hidden when switched off', () => {
		const events = [text('thought', 1, 'thinking'), text('answer', 2)];
		expect(roles(events)).toEqual(['thinking', 'assistant']);
		expect(roles(events, ctx({ thinking: false }))).toEqual(['assistant']);
	});

	it('is the only role left when everything else is off', () => {
		const events = [text('thought', 1, 'thinking'), text('answer', 2)];
		expect(roles(events, only('thinking'))).toEqual(['thinking']);
	});

	it('does not take a turn number or a fork anchor', () => {
		const lines = buildLines([text('▷ User: go', 1), text('thought', 2, 'thinking')], ctx());
		expect(lines[1].turn).toBeUndefined();
		expect(lines[1].messageId).toBeUndefined();
	});
});

describe('turn summaries', () => {
	it('attaches to the preceding assistant line instead of adding one', () => {
		const lines = buildLines([text('done', 1), summary('Refactored the parser', 2)], ctx());
		expect(lines).toHaveLength(1);
		expect(lines[0].role).toBe('assistant');
		expect(lines[0].summary).toEqual({
			detail: 'Refactored the parser',
			needsAction: false,
			ts: 2
		});
	});

	it('carries needs_action through', () => {
		const lines = buildLines(
			[text('done', 1), summary('Waiting on a decision', 2, { needsAction: true })],
			ctx()
		);
		expect(lines[0].summary?.needsAction).toBe(true);
	});

	it('falls back to status_category when detail is empty', () => {
		const lines = buildLines([text('done', 1), summary('  ', 2, { category: 'blocked' })], ctx());
		expect(lines[0].summary?.detail).toBe('blocked');
	});

	it('drops a summary with neither detail nor category', () => {
		const lines = buildLines([text('done', 1), summary('', 2, { category: '  ' })], ctx());
		expect(lines).toHaveLength(1);
		expect(lines[0].summary).toBeUndefined();
	});

	it('skips back over tool lines to the turn’s last assistant bubble', () => {
		const toolCall: AgentEvent = { type: 'tool_call', tool: 'Bash', input: {}, ts: 2, seq: null };
		const toolResult: AgentEvent = {
			type: 'tool_result',
			tool: 'Bash',
			output_summary: 'ok',
			error: false,
			ts: 3,
			seq: null
		};
		const lines = buildLines(
			[text('running it', 1), toolCall, toolResult, summary('Ran the suite', 4)],
			ctx()
		);
		expect(lines.map((l) => l.role)).toEqual(['assistant', 'tool', 'result']);
		expect(lines[0].summary?.detail).toBe('Ran the suite');
	});

	it('stands alone when no assistant line opened the turn', () => {
		const lines = buildLines([text('▷ User: go', 1), summary('Nothing to do', 2)], ctx());
		expect(lines.map((l) => l.role)).toEqual(['user', 'summary']);
		expect(lines[1].summary?.detail).toBe('Nothing to do');
	});

	it('stands alone rather than overwriting an assistant line that already has one', () => {
		const lines = buildLines(
			[text('done', 1), summary('first', 2), summary('second', 3)],
			ctx()
		);
		expect(lines.map((l) => l.role)).toEqual(['assistant', 'summary']);
		expect(lines[0].summary?.detail).toBe('first');
		expect(lines[1].summary?.detail).toBe('second');
	});

	it('is hidden when switched off, leaving the assistant bubble untouched', () => {
		const events = [text('done', 1), summary('Refactored the parser', 2)];
		const lines = buildLines(events, ctx({ summary: false }));
		expect(lines).toHaveLength(1);
		expect(lines[0].summary).toBeUndefined();
	});

	it('survives an assistant line hidden by the filter, as a standalone footer', () => {
		const events = [text('done', 1), summary('Refactored the parser', 2)];
		const lines = buildLines(events, ctx({ assistant: false }));
		expect(lines.map((l) => l.role)).toEqual(['summary']);
	});

	it('stays invisible to the consecutive-duplicate guard when attached', () => {
		const events = [text('same', 1), summary('s', 2), text('same', 3)];
		expect(roles(events)).toEqual(['assistant']);
	});
});
