import { describe, expect, it } from 'vitest';
import type { AgentEvent } from '@bindings/AgentEvent';
import { allFilter } from './filters';
import { resultText, textBlocksText, unescapeJsonText } from './format';
import { buildLines, type LineBuildCtx } from './lines';

const ctx: LineBuildCtx = {
	visible: (c) => allFilter(true)[c],
	renderMarkdown: (s) => `<p>${s}</p>`,
	renderCode: (text) => `<code>${text}</code>`,
	prettyJson: true,
	prettyDiff: true
};

const result = (output: string): AgentEvent => ({
	type: 'tool_result',
	tool: 'Agent',
	output_summary: output,
	kind: null,
	error: false,
	ts: 1,
	seq: null
});

const user = (content: string): AgentEvent => ({
	type: 'text',
	content: `▷ User: ${content}`,
	meta: false,
	kind: null,
	ts: 1,
	message_id: null,
	usage: null,
	seq: null
});

describe('textBlocksText', () => {
	it('joins text blocks with real newlines', () => {
		const raw = JSON.stringify([
			{ type: 'text', text: 'Async agent launched.\nagentId: a1' },
			{ type: 'text', text: 'second' }
		]);
		expect(textBlocksText(raw)).toBe('Async agent launched.\nagentId: a1\n\nsecond');
	});

	it('leaves anything that is not purely text blocks alone', () => {
		expect(textBlocksText('[1, 2]')).toBeNull();
		expect(textBlocksText('[{"type":"image","source":{}}]')).toBeNull();
		expect(textBlocksText('[]')).toBeNull();
		expect(textBlocksText('[not json "text"]')).toBeNull();
		expect(textBlocksText('plain output')).toBeNull();
	});
});

describe('unescapeJsonText', () => {
	it('decodes a whole JSON string literal', () => {
		expect(unescapeJsonText('"line one\\nline \\"two\\""')).toBe('line one\nline "two"');
	});

	it('unescapes text where every quote is escaped', () => {
		expect(unescapeJsonText('ship \\"fr and ko behind\\" the flag')).toBe(
			'ship "fr and ko behind" the flag'
		);
	});

	it('keeps text that mixes bare and escaped quotes verbatim', () => {
		const code = 'echo "a \\"b\\" c"';
		expect(unescapeJsonText(code)).toBe(code);
	});

	it('keeps text without escapes verbatim', () => {
		expect(unescapeJsonText('C:\\path\\to')).toBe('C:\\path\\to');
	});
});

describe('tool result and user lines', () => {
	it('renders an Agent result block array as plain text', () => {
		const raw = JSON.stringify([{ type: 'text', text: 'Async agent launched\nagentId: x' }]);
		const [ln] = buildLines([result(raw)], ctx);
		expect(ln.role).toBe('result');
		expect(ln.text).toBe('Async agent launched\nagentId: x');
		expect(ln.text).not.toContain('"type"');
	});

	it('resultText falls back to the raw summary', () => {
		expect(resultText('total 4\ndrwx')).toBe('total 4\ndrwx');
	});

	it('shows a double-encoded user prompt without backslashes', () => {
		const [ln] = buildLines([user('keep \\"fr and ko behind\\" a flag')], ctx);
		expect(ln.role).toBe('user');
		expect(ln.text).toBe('keep "fr and ko behind" a flag');
	});
});
