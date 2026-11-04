import { describe, expect, it } from 'vitest';
import { straightenPhraseQuotes, tokenizeQuery } from './search';

describe('straightenPhraseQuotes', () => {
	it('quotes a phrase pasted between smart quotes like a typed one', () => {
		const q = straightenPhraseQuotes('“master...HEAD (three dots).” gate');
		expect(q).toBe('"master...HEAD (three dots)." gate');
		expect(tokenizeQuery(q)).toEqual(['master...HEAD (three dots).', 'gate']);
		expect(straightenPhraseQuotes('„so“')).toBe('"so"');
	});

	it('keeps smart quotes inside a straight-quoted phrase as text', () => {
		const q = '"It’s “quoted” text"';
		expect(straightenPhraseQuotes(q)).toBe(q);
		expect(straightenPhraseQuotes('plain words')).toBe('plain words');
	});
});
