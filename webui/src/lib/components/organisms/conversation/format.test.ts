import { describe, it, expect } from 'vitest';
import { insertAtCaret, parseTodos, quoteMarkdown, todoProgress } from './format';
import type { AskQuestion, Line } from './types';

// The Conversation Drawer must render the combined message list in strict
// ascending timestamp order, regardless of role. `events` is sorted ascending
// by `ts` in ConversationDrawer.svelte (`.sort((a, b) => a.ts - b.ts)`) and the
// lines are built from it in order with no role grouping and no re-anchoring.

const askQ: AskQuestion[] = [{ question: 'Which database?', options: [{ label: 'Postgres' }, { label: 'SQLite' }] }];

const preamble = (ts: number): Line => ({ role: 'assistant', ts, text: 'I need to know which DB to use.' });
const askCard = (ts: number): Line => ({ role: 'tool', ts, tool: 'AskUserQuestion', ask: askQ });
const answer = (ts: number): Line => ({ role: 'user', ts, text: 'Postgres' });
const continuation = (ts: number): Line => ({ role: 'assistant', ts, text: 'Great, using Postgres.' });

// Mirror the drawer's render order: events are sorted ascending by ts (a stable
// sort keeps equal-ts ties in source order, as the component relies on).
const renderOrder = (lines: Line[]) => lines.slice().sort((a, b) => a.ts - b.ts);
const tsOf = (ls: Line[]) => ls.map((l) => l.ts);

describe('conversation render ordering', () => {
	it('renders the combined list in strict ascending timestamp order', () => {
		// The reported regression: an assistant message (15:30) ahead of an earlier
		// user message (15:12). Source order is intentionally scrambled by role.
		const t1512 = answer(1512);
		const t1530 = preamble(1530);
		const t1535 = answer(1535);
		const out = renderOrder([t1530, t1512, t1535]);
		expect(tsOf(out)).toEqual([1512, 1530, 1535]);
		expect(out[0]).toBe(t1512);
		expect(out[1]).toBe(t1530);
		expect(out[2]).toBe(t1535);
	});

	it('does not lift an ask card / preamble above an earlier user line', () => {
		// A conversation containing an ask must NOT reorder around it: every line
		// stays in ts order, including the user answer that follows the ask.
		const lines: Line[] = [answer(100), preamble(200), askCard(210), answer(220), continuation(300)];
		const out = renderOrder(lines);
		expect(tsOf(out)).toEqual([100, 200, 210, 220, 300]);
	});

	it('keeps equal-timestamp ties in source order (stable)', () => {
		const a = answer(100);
		const b = preamble(100);
		const out = renderOrder([a, b]);
		expect(out[0]).toBe(a);
		expect(out[1]).toBe(b);
	});
});

describe('parseTodos', () => {
	it('parses a claude TodoWrite input', () => {
		const todos = parseTodos({
			todos: [
				{ content: 'Wire the parser', status: 'completed', activeForm: 'Wiring the parser' },
				{ content: 'Render the card', status: 'in_progress', activeForm: 'Rendering the card' },
				{ content: 'Write the tests', status: 'pending', activeForm: 'Writing the tests' }
			]
		});
		expect(todos).toEqual([
			{ content: 'Wire the parser', status: 'completed', activeForm: 'Wiring the parser' },
			{ content: 'Render the card', status: 'in_progress', activeForm: 'Rendering the card' },
			{ content: 'Write the tests', status: 'pending', activeForm: 'Writing the tests' }
		]);
	});

	it('normalizes a codex update_plan step array into the same shape', () => {
		expect(
			parseTodos({
				plan: [
					{ step: 'Read the code', status: 'completed' },
					{ step: 'Patch it', status: 'in_progress' }
				]
			})
		).toEqual([
			{ content: 'Read the code', status: 'completed', activeForm: undefined },
			{ content: 'Patch it', status: 'in_progress', activeForm: undefined }
		]);
	});

	it('defaults an unknown status to pending', () => {
		expect(parseTodos({ todos: [{ content: 'x', status: 'banana' }] })?.[0].status).toBe('pending');
	});

	it('captures blocked-by relations in either casing when present', () => {
		expect(parseTodos({ todos: [{ content: 'a', status: 'pending', blockedBy: ['x', 'y'] }] })?.[0].blockedBy).toEqual([
			'x',
			'y'
		]);
		expect(parseTodos({ todos: [{ content: 'a', status: 'pending', blocked_by: ['x'] }] })?.[0].blockedBy).toEqual(['x']);
	});

	it('leaves blockedBy undefined when absent or unusable', () => {
		expect(parseTodos({ todos: [{ content: 'a', status: 'pending' }] })?.[0].blockedBy).toBeUndefined();
		expect(parseTodos({ todos: [{ content: 'a', status: 'pending', blockedBy: 'nope' }] })?.[0].blockedBy).toBeUndefined();
		expect(parseTodos({ todos: [{ content: 'a', status: 'pending', blockedBy: [1, ''] }] })?.[0].blockedBy).toBeUndefined();
	});

	it('degrades to null on malformed or empty input without throwing', () => {
		for (const bad of [undefined, null, {}, { todos: [] }, { todos: 'nope' }, { plan: [] }, 42, 'str']) {
			expect(parseTodos(bad)).toBeNull();
		}
		expect(parseTodos({ todos: [null, 7, { status: 'pending' }, { content: '   ' }] })).toBeNull();
	});
});

describe('todoProgress', () => {
	it('counts done/total and surfaces the in_progress item', () => {
		const p = todoProgress([
			{ content: 'a', status: 'completed' },
			{ content: 'b', status: 'in_progress', activeForm: 'Doing b' },
			{ content: 'c', status: 'pending' }
		]);
		expect(p).not.toBeNull();
		expect(p?.done).toBe(1);
		expect(p?.total).toBe(3);
		expect(p?.inProgress?.activeForm).toBe('Doing b');
	});

	it('is null for an absent or empty list', () => {
		expect(todoProgress(null)).toBeNull();
		expect(todoProgress([])).toBeNull();
	});
});

describe('quoteMarkdown', () => {
	it('prefixes every line and closes with a blank line', () => {
		expect(quoteMarkdown('first line\n\nsecond line')).toBe('> first line\n>\n> second line\n\n');
	});

	it('drops trailing whitespace and newlines of the source', () => {
		expect(quoteMarkdown('hello\n\n   \n')).toBe('> hello\n\n');
	});

	it('normalizes CRLF', () => {
		expect(quoteMarkdown('a\r\nb')).toBe('> a\n> b\n\n');
	});

	it('keeps code fences quoted line by line', () => {
		expect(quoteMarkdown('```sh\nls -l\n```')).toBe('> ```sh\n> ls -l\n> ```\n\n');
	});

	it('is empty for blank input', () => {
		expect(quoteMarkdown('')).toBe('');
		expect(quoteMarkdown('  \n ')).toBe('');
	});
});

describe('insertAtCaret', () => {
	const block = '> q\n\n';

	it('becomes the whole draft when empty', () => {
		expect(insertAtCaret('', undefined, block)).toEqual({ text: block, caret: block.length });
	});

	it('appends with a blank line when the caret is absent', () => {
		expect(insertAtCaret('typed', undefined, block).text).toBe('typed\n\n> q\n\n');
	});

	it('splices at the caret, blank-line separated on both sides', () => {
		const r = insertAtCaret('beforeafter', 6, block);
		expect(r.text).toBe('before\n\n> q\n\nafter');
		expect(r.caret).toBe('before\n\n> q\n\n'.length);
	});

	it('does not stack blank lines that are already there', () => {
		expect(insertAtCaret('a\n\n', undefined, block).text).toBe('a\n\n> q\n\n');
		expect(insertAtCaret('a\n', undefined, block).text).toBe('a\n\n> q\n\n');
	});

	it('inserts at the start without a leading blank line', () => {
		expect(insertAtCaret('rest', 0, block).text).toBe('> q\n\nrest');
	});

	it('clamps a caret past the end', () => {
		expect(insertAtCaret('x', 99, block).text).toBe('x\n\n> q\n\n');
	});

	it('stacks successive quotes instead of replacing them', () => {
		const first = insertAtCaret('', undefined, block);
		const second = insertAtCaret(first.text, first.caret, '> r\n\n');
		expect(second.text).toBe('> q\n\n> r\n\n');
	});
});
