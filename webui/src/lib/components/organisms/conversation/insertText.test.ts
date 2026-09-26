import { describe, expect, it } from 'vitest';
import { insertBlock } from './insertText';

describe('insertBlock', () => {
	it('appends to an empty draft and leaves the caret on the next line', () => {
		expect(insertBlock('', 'B')).toEqual({ value: 'B\n', caret: 2 });
	});

	it('separates from an existing draft with a blank line', () => {
		expect(insertBlock('hello', 'B')).toEqual({ value: 'hello\n\nB\n', caret: 9 });
		expect(insertBlock('hello\n', 'B').value).toBe('hello\n\nB\n');
		expect(insertBlock('hello\n\n', 'B').value).toBe('hello\n\nB\n');
	});

	it('inserts at the caret and keeps what follows', () => {
		const r = insertBlock('ab', 'B', 1);
		expect(r.value).toBe('a\n\nB\n\nb');
		expect(r.caret).toBe(6);
		expect(insertBlock('a\n\nb', 'B', 3).value).toBe('a\n\nB\n\nb');
		expect(insertBlock('x', 'B', 99).value).toBe('x\n\nB\n');
	});
});
