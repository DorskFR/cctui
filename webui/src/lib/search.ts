import { type Query, walk } from '@dorsk/tsumikit';

/**
 * Search-term parsing + highlighting.
 *
 * `tokenizeQuery` mirrors the server's tokenizer (admin.rs `tokenize_query`):
 * whitespace-split into terms, but a `"…"`-quoted span stays a single exact
 * term (spaces preserved). Terms are AND-matched server-side; the client uses
 * the same split to highlight every term in snippets and the chat window.
 */
export function tokenizeQuery(q: string): string[] {
	return tokens(q).map((t) => t.text);
}

type Token = { text: string; quoted: boolean };

function tokens(q: string): Token[] {
	const out: Token[] = [];
	let cur = '';
	let inQuote = false;
	const flush = () => {
		if (cur) out.push({ text: cur, quoted: inQuote });
		cur = '';
	};
	for (const c of q) {
		if (c === '"') {
			flush();
			inQuote = !inQuote;
		} else if (/\s/.test(c) && !inQuote) {
			flush();
		} else {
			cur += c;
		}
	}
	flush();
	return out.slice(0, 8);
}

/** Marks a highlight term that only counts where it starts a word. */
const WORD_START = '\u0001';
const WORD_CHAR = /^[\p{L}\p{N}_]/u;

/** `term` as a highlight term that must start a word. */
export const wordStart = (term: string): string => WORD_START + term;

/** The session search's rule, as the server's `search_term`: a bare term
 *  starts a word, a quoted one or a `*term` matches anywhere, a trailing `*`
 *  is dropped, and a term whose first character cannot start a word matches
 *  anywhere. */
function anchored({ text, quoted }: Token): string {
	if (quoted) return text;
	const bare = text.endsWith('*') ? text.slice(0, -1) : text;
	if (bare.length > 1 && bare.startsWith('*')) return bare.slice(1);
	if (!bare) return text;
	return WORD_CHAR.test(bare) ? wordStart(bare) : bare;
}

function escapeRegExp(s: string): string {
	return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

function termPattern(term: string): string {
	return term.startsWith(WORD_START)
		? `(?<![\\p{L}\\p{N}_])${escapeRegExp(term.slice(WORD_START.length))}`
		: escapeRegExp(term);
}

const termLength = (term: string) =>
	term.length - (term.startsWith(WORD_START) ? WORD_START.length : 0);

/**
 * Wrap every occurrence of any term in `<mark class="search-hit">`, touching
 * only text *outside* HTML tags so it never corrupts the markdown/highlight
 * markup it's layered over. Case-insensitive; longest terms first so a term
 * that is a prefix of another doesn't shadow it.
 */
export function highlightTerms(html: string, terms: string[]): string {
	const pat = terms
		.filter((t) => termLength(t) > 0)
		.sort((a, b) => termLength(b) - termLength(a))
		.map(termPattern)
		.join('|');
	if (!pat) return html;
	const re = new RegExp(`(${pat})`, 'giu');
	// Split into tags (`<…>`) and text runs; only the runs get highlighted.
	return html.replace(/<[^>]*>|[^<]+/g, (seg) =>
		seg.startsWith('<') ? seg : seg.replace(re, '<mark class="search-hit">$1</mark>')
	);
}

const SMART_DOUBLE_QUOTES = /[“”„‟]/g;

/** Keep in step with the server's `straighten_phrase_quotes`. */
export function straightenPhraseQuotes(q: string): string {
	return q.includes('"') ? q : q.replace(SMART_DOUBLE_QUOTES, '"');
}

/** Free-text terms of `q`, parsed from `input`. The parser merges adjacent
 *  words and phrases into one text node, so each node's source span is
 *  re-tokenised: a quoted phrase stays one term, a bare word is its own. */
export function queryTerms(q: Query, input: string): string[] {
	return textTokens(q, input).map((t) => t.text);
}

/** {@link queryTerms} for the session search: bare terms highlight only where
 *  they start a word, as the server matches them. */
export function sessionQueryTerms(q: Query, input: string): string[] {
	return textTokens(q, input).map(anchored);
}

function textTokens(q: Query, input: string): Token[] {
	const out: Token[] = [];
	walk(q.root, (n) => {
		if (n.kind === 'text') out.push(...tokens(input.slice(n.span[0], n.span[1])));
	});
	return out.slice(0, 8);
}
