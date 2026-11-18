import { describe, expect, it } from 'vitest';
import { parse } from '@dorsk/tsumikit';
import { buildConversationSearchSchema } from './components/organisms/conversation/searchSchema';
import {
	highlightTerms,
	queryTerms,
	sessionQueryTerms,
	straightenPhraseQuotes,
	tokenizeQuery,
	wordStart
} from './search';
import { buildSessionSearchSchema } from './searchSchema';

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

describe('sessionQueryTerms', () => {
	const schema = buildSessionSearchSchema(async () => []);
	const terms = (input: string) => sessionQueryTerms(parse(input, schema), input);

	it('anchors bare terms at a word start, as the server does', () => {
		expect(terms('kanade hook')).toEqual([wordStart('kanade'), wordStart('hook')]);
		expect(terms('hook*')).toEqual([wordStart('hook')]);
	});

	it('leaves quoted, star-prefixed and non-word-start terms unanchored', () => {
		expect(terms('"use hook" *hook *hook* .env')).toEqual(['use hook', 'hook', 'hook', '.env']);
		expect(terms('*')).toEqual(['*']);
	});

	it('leaves the conversation terms plain', () => {
		const conv = buildConversationSearchSchema(() => []);
		expect(queryTerms(parse('hook', conv), 'hook')).toEqual(['hook']);
	});
});

describe('highlightTerms', () => {
	const mark = (s: string) => `<mark class="search-hit">${s}</mark>`;

	it('marks a word-start term only where it starts a word', () => {
		const out = highlightTerms('lefthook, useHook, Hooks and (hook)', [wordStart('hook')]);
		expect(out).toBe(`lefthook, useHook, ${mark('Hook')}s and (${mark('hook')})`);
	});

	it('keeps plain terms as substring matches', () => {
		expect(highlightTerms('lefthook', ['hook'])).toBe(`left${mark('hook')}`);
	});

	it('treats letters beyond ASCII and _ as word characters', () => {
		expect(highlightTerms('caf\u00e9hook use_hook é hook', [wordStart('hook')])).toBe(
			`caf\u00e9hook use_hook é ${mark('hook')}`
		);
	});

	it('never touches markup and prefers the longest term', () => {
		const html = '<span class="hook">hook hooks</span>';
		expect(highlightTerms(html, [wordStart('hook'), wordStart('hooks')])).toBe(
			`<span class="hook">${mark('hook')} ${mark('hooks')}</span>`
		);
		expect(highlightTerms('text', [wordStart('')])).toBe('text');
	});
});
