import { describe, expect, it } from 'vitest';
import {
	USER_REDACTED,
	hasUnclosedTag,
	redactStoredText,
	redactUserSpans,
	redactedSegments,
	redactedSpans,
	wrapRedacted
} from './redactTag';
import type { AgentEvent } from '@bindings/AgentEvent';
import { USER_PREFIX, userMsgKey } from './ws/frames';

describe('redactUserSpans', () => {
	it('masks a single span, tags included', () => {
		expect(redactUserSpans('pw is <redacted>hunter2</redacted>!')).toBe(`pw is ${USER_REDACTED}!`);
	});

	it('masks several spans', () => {
		expect(redactUserSpans('<redacted>a1</redacted> and <redacted>b2</redacted>')).toBe(
			`${USER_REDACTED} and ${USER_REDACTED}`
		);
	});

	it('masks a multiline span', () => {
		expect(redactUserSpans('x <redacted>line1\nline2\r\nline3</redacted> y')).toBe(
			`x ${USER_REDACTED} y`
		);
	});

	it('matches tag names case-insensitively', () => {
		expect(redactUserSpans('<REDACTED>s1</Redacted> <ReDaCtEd>s2</rEdAcTeD>')).toBe(
			`${USER_REDACTED} ${USER_REDACTED}`
		);
	});

	it('leaves an unclosed tag as written', () => {
		const t = 'keep <redacted>secret and more';
		expect(redactUserSpans(t)).toBe(t);
	});

	it('leaves an empty pair as written', () => {
		const t = 'a <redacted></redacted> b';
		expect(redactUserSpans(t)).toBe(t);
	});

	it('closes on the first closing tag (no nesting)', () => {
		expect(redactUserSpans('<redacted>a<redacted>b</redacted>c</redacted>')).toBe(
			`${USER_REDACTED}c</redacted>`
		);
	});

	it('is idempotent', () => {
		const once = redactUserSpans('x <redacted>v</redacted> y <redacted>open');
		expect(redactUserSpans(once)).toBe(once);
		expect(redactUserSpans(USER_REDACTED)).toBe(USER_REDACTED);
	});
});

describe('redactedSpans', () => {
	it('reports offsets and values of closed non-empty spans only', () => {
		const t = 'a<redacted>v1</redacted>b<redacted></redacted><redacted>v2</REDACTED>';
		const spans = redactedSpans(t);
		expect(spans.map((s) => s.value)).toEqual(['v1', 'v2']);
		expect(t.slice(spans[0].start, spans[0].end)).toBe('<redacted>v1</redacted>');
	});
});

describe('hasUnclosedTag', () => {
	it('flags an open tag without its close', () => {
		expect(hasUnclosedTag('x <redacted>oops')).toBe(true);
		expect(hasUnclosedTag('<redacted>ok</redacted> then <REDACTED>oops')).toBe(true);
	});

	it('is quiet for closed or absent tags', () => {
		expect(hasUnclosedTag('plain')).toBe(false);
		expect(hasUnclosedTag('<redacted>ok</redacted>')).toBe(false);
		expect(hasUnclosedTag('<redacted></redacted>')).toBe(false);
	});
});

describe('wrapRedacted', () => {
	it('wraps the selection and keeps it selected', () => {
		const out = wrapRedacted('my pw hunter2 ok', 6, 13);
		expect(out.text).toBe('my pw <redacted>hunter2</redacted> ok');
		expect(out.text.slice(out.selStart, out.selEnd)).toBe('hunter2');
	});

	it('inserts an empty pair at a collapsed caret', () => {
		const out = wrapRedacted('ab', 1, 1);
		expect(out.text).toBe('a<redacted></redacted>b');
		expect(out.selStart).toBe(out.selEnd);
		expect(out.selStart).toBe(11);
	});
});

describe('redactStoredText', () => {
	it('masks plain text', () => {
		expect(redactStoredText('hi <redacted>pw</redacted>')).toBe(`hi ${USER_REDACTED}`);
	});

	it('masks each string of a JSON document without breaking it', () => {
		const doc = JSON.stringify({
			prompt: 'use <redacted>tok"en\nx</redacted> now',
			name: 'n',
			envRows: [{ key: 'K', value: '<redacted>v</redacted>' }]
		});
		const out = redactStoredText(doc);
		expect(out).not.toContain('tok');
		const parsed = JSON.parse(out);
		expect(parsed.prompt).toBe(`use ${USER_REDACTED} now`);
		expect(parsed.envRows[0].value).toBe(USER_REDACTED);
	});

	it('cannot join a span across two JSON fields', () => {
		const doc = JSON.stringify({ a: 'x <redacted>', b: '</redacted> y' });
		expect(redactStoredText(doc)).toBe(doc);
	});

	it('masks a history list', () => {
		const doc = JSON.stringify(['first', 'second <redacted>s3cret</redacted>']);
		expect(JSON.parse(redactStoredText(doc))).toEqual(['first', `second ${USER_REDACTED}`]);
	});

	it('returns text without tags untouched', () => {
		const doc = '{"a": 1}';
		expect(redactStoredText(doc)).toBe(doc);
	});
});

describe('optimistic echo reconciliation', () => {
	it('keys the typed reply and the daemon-masked echo as the same message', () => {
		const typed = { type: 'reply', content: 'go <redacted>hunter2-xyz</redacted>', ts: 1 } as AgentEvent;
		const stored = {
			type: 'text',
			content: `${USER_PREFIX} go ${USER_REDACTED}`,
			ts: 2
		} as unknown as AgentEvent;
		expect(userMsgKey(typed)).toBe(userMsgKey(stored));
		expect(userMsgKey(typed)).not.toContain('hunter2-xyz');
	});
});

describe('redactedSegments', () => {
	const joined = (t: string) =>
		redactedSegments(t)
			.map((s) => s.text)
			.join('');
	const masked = (t: string, revealed?: Set<string>) =>
		redactedSegments(t, revealed)
			.filter((s) => s.mask)
			.map((s) => s.text);

	it('masks only the inside of a closed span, tags stay plain', () => {
		expect(redactedSegments('pw <redacted>hunter2</redacted>!')).toEqual([
			{ text: 'pw <redacted>' },
			{ text: 'hunter2', mask: true },
			{ text: '</redacted>!' }
		]);
	});

	it('returns nothing to mask for plain or empty text', () => {
		expect(redactedSegments('')).toEqual([]);
		expect(redactedSegments('plain')).toEqual([{ text: 'plain' }]);
	});

	it('leaves an unclosed tag unmasked, since it is sent as written', () => {
		const t = 'x <redacted>oops';
		expect(masked(t)).toEqual([]);
		expect(joined(t)).toBe(t);
	});

	it('masks a closed span before an unclosed one', () => {
		const t = '<redacted>a1</redacted> then <redacted>oops';
		expect(masked(t)).toEqual(['a1']);
		expect(joined(t)).toBe(t);
	});

	it('splits adjacent spans and keeps case-insensitive tags', () => {
		const t = '<redacted>a</redacted><REDACTED>b</REDACTED>';
		expect(redactedSegments(t)).toEqual([
			{ text: '<redacted>' },
			{ text: 'a', mask: true },
			{ text: '</redacted><REDACTED>' },
			{ text: 'b', mask: true },
			{ text: '</REDACTED>' }
		]);
	});

	it('masks a nested span up to the first close, like the server', () => {
		const t = '<redacted>a<redacted>b</redacted>c</redacted>';
		expect(masked(t)).toEqual(['a<redacted>b']);
		expect(joined(t)).toBe(t);
	});

	it('leaves an empty span unmasked', () => {
		expect(masked('<redacted></redacted>')).toEqual([]);
	});

	it('unmasks revealed values only', () => {
		const t = '<redacted>a1</redacted> <redacted>b2</redacted>';
		expect(masked(t, new Set(['b2']))).toEqual(['a1']);
	});

	it('round-trips multi-line text byte for byte', () => {
		const t = 'line1\r\n<redacted>multi\nline</redacted>\ttail 🔑';
		expect(joined(t)).toBe(t);
		expect(masked(t)).toEqual(['multi\nline']);
	});
});
