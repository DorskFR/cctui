import { describe, expect, it } from 'vitest';
import { foldSearchText, markdownToSearchText } from '$lib/searchText';
import { parityFixture } from './fixtures';

type Fixture = {
	markdownText: { md: string; out: string }[];
	fold: { s: string; out: string }[];
};

const fx = parityFixture<Fixture>('searchText');

describe('searchText parity fixtures', () => {
	it('markdownToSearchText', () => {
		for (const c of fx.markdownText) expect(markdownToSearchText(c.md), JSON.stringify(c)).toBe(c.out);
	});
	it('foldSearchText', () => {
		for (const c of fx.fold) expect(foldSearchText(c.s), JSON.stringify(c)).toBe(c.out);
	});
});
