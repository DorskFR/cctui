import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { buildLines, type LineBuildCtx } from './lines';
import { MSG_TYPES, msgTypeLabel, type MsgType, type TagState } from './types';

const ctx = (filter: Partial<Record<MsgType, TagState>> = {}): LineBuildCtx => {
	const state = (t: MsgType): TagState => filter[t] ?? 'off';
	const anyIncluded = (Object.values(filter) as TagState[]).some((s) => s === 'include');
	return {
		typeVisible: (t) => {
			if (state(t) === 'exclude') return false;
			if (anyIncluded) return state(t) === 'include';
			return true;
		},
		renderMarkdown: (s) => `<p>${s}</p>`,
		renderCode: (text) => `<code>${text}</code>`,
		prettyJson: true,
		prettyDiff: true
	};
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

describe('filter categories', () => {
	it('carries a translated label for every tag, including the new ones', () => {
		expect(MSG_TYPES.map((t) => t.id)).toContain('thinking');
		expect(MSG_TYPES.map((t) => t.id)).toContain('summary');
		for (const t of MSG_TYPES) expect(msgTypeLabel(t.id)).toBeTruthy();
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

	it('leaves unknown kinds on their existing role', () => {
		expect(roles([text('a file', 1, 'attachment')])).toEqual(['assistant']);
		expect(roles([text('▷ User: hi', 1, 'system_marker')])).toEqual(['user']);
	});

	it('is shown by default and hidden when excluded', () => {
		const events = [text('thought', 1, 'thinking'), text('answer', 2)];
		expect(roles(events)).toEqual(['thinking', 'assistant']);
		expect(roles(events, ctx({ thinking: 'exclude' }))).toEqual(['assistant']);
	});

	it('is the only role left when thinking is the sole include', () => {
		const events = [text('thought', 1, 'thinking'), text('answer', 2)];
		expect(roles(events, ctx({ thinking: 'include' }))).toEqual(['thinking']);
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

	it('is hidden when excluded, leaving the assistant bubble untouched', () => {
		const events = [text('done', 1), summary('Refactored the parser', 2)];
		const lines = buildLines(events, ctx({ summary: 'exclude' }));
		expect(lines).toHaveLength(1);
		expect(lines[0].summary).toBeUndefined();
	});

	it('survives an assistant line hidden by the filter, as a standalone footer', () => {
		const events = [text('done', 1), summary('Refactored the parser', 2)];
		const lines = buildLines(events, ctx({ assistant: 'exclude' }));
		expect(lines.map((l) => l.role)).toEqual(['summary']);
	});

	it('stays invisible to the consecutive-duplicate guard when attached', () => {
		const events = [text('same', 1), summary('s', 2), text('same', 3)];
		expect(roles(events)).toEqual(['assistant']);
	});
});
