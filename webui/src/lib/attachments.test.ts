import { describe, expect, it } from 'vitest';
import { appendFileTokens } from './attachments';

const f = (name: string) => new File(['x'], name, { type: 'text/plain' });

describe('appendFileTokens', () => {
	it('appends a bracketed token per file to empty text', () => {
		expect(appendFileTokens('', [f('a.png'), f('b.csv')])).toBe('[a.png] [b.csv]');
	});

	it('separates from existing text with a space', () => {
		expect(appendFileTokens('see this:', [f('a.png')])).toBe('see this: [a.png]');
	});

	it('does not double a trailing space or newline', () => {
		expect(appendFileTokens('line one\n', [f('a.png')])).toBe('line one\n[a.png]');
		expect(appendFileTokens('word ', [f('a.png')])).toBe('word [a.png]');
	});

	it('skips names already referenced in the text', () => {
		expect(appendFileTokens('about [a.png] here', [f('a.png'), f('b.csv')])).toBe(
			'about [a.png] here [b.csv]'
		);
	});

	it('is idempotent on a re-pick of the same file', () => {
		const once = appendFileTokens('', [f('a.png')]);
		expect(appendFileTokens(once, [f('a.png')])).toBe(once);
	});
});
