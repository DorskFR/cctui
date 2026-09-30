import { describe, expect, it } from 'vitest';
import type { Bookmark } from '@bindings/Bookmark';
import { bookmarkMarkdown, defaultTitle, isDeadLink, queryTerms, sourceHref } from '$lib/bookmarks';
import { parityFixture } from './fixtures';

type Fixture = {
	defaultTitle: { body: string; out: string }[];
	queryTerms: { q: string; out: string[] }[];
	sourceHref: { session_id: string | null; seq: number | null; out: string | null }[];
	isDeadLink: { session_id: string | null; out: boolean }[];
	bookmarkMarkdown: { title: string; note: string | null; body: string; out: string }[];
};

const fx = parityFixture<Fixture>('bookmarks');

const bm = (p: Partial<Bookmark>): Bookmark => p as Bookmark;

describe('bookmarks parity fixtures', () => {
	it('defaultTitle', () => {
		for (const c of fx.defaultTitle) expect(defaultTitle(c.body), JSON.stringify(c)).toBe(c.out);
	});
	it('queryTerms', () => {
		for (const c of fx.queryTerms) expect(queryTerms(c.q), JSON.stringify(c)).toEqual(c.out);
	});
	it('sourceHref', () => {
		for (const c of fx.sourceHref)
			expect(sourceHref(bm({ session_id: c.session_id, seq: c.seq })), JSON.stringify(c)).toBe(c.out);
	});
	it('isDeadLink', () => {
		for (const c of fx.isDeadLink)
			expect(isDeadLink(bm({ session_id: c.session_id })), JSON.stringify(c)).toBe(c.out);
	});
	it('bookmarkMarkdown', () => {
		for (const c of fx.bookmarkMarkdown)
			expect(
				bookmarkMarkdown(bm({ title: c.title, note: c.note, body: c.body })),
				JSON.stringify(c)
			).toBe(c.out);
	});
});
