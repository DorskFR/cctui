import { describe, expect, it } from 'vitest';
import { highlightTerms, tokenizeQuery } from '$lib/search';
import { parityFixture } from './fixtures';

type Fixture = {
	tokenizeQuery: { q: string; out: string[] }[];
	highlightTerms: { html: string; terms: string[]; out: string }[];
};

const fx = parityFixture<Fixture>('search');

describe('search parity fixtures', () => {
	it('tokenizeQuery', () => {
		for (const c of fx.tokenizeQuery) expect(tokenizeQuery(c.q), JSON.stringify(c)).toEqual(c.out);
	});
	it('highlightTerms', () => {
		for (const c of fx.highlightTerms)
			expect(highlightTerms(c.html, c.terms), JSON.stringify(c)).toBe(c.out);
	});
});
