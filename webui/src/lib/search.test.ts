import { describe, expect, it } from 'vitest';
import { parse } from '@dorsk/tsumikit';
import { buildConversationSearchSchema } from './components/organisms/conversation/searchSchema';
import { queryTerms, straightenPhraseQuotes, tokenizeQuery } from './search';

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

describe('queryTerms', () => {
	it('keeps a quoted phrase as one term and skips field clauses', () => {
		const schema = buildConversationSearchSchema(() => []);
		const input = 'role:user "master...HEAD (three dots)." gate';
		expect(queryTerms(parse(input, schema), input)).toEqual([
			'master...HEAD (three dots).',
			'gate'
		]);
		const words = 'foo "a b" bar';
		expect(queryTerms(parse(words, schema), words)).toEqual(['foo', 'a b', 'bar']);
	});
});
