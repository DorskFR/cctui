/**
 * Search-term parsing + highlighting.
 *
 * `tokenizeQuery` mirrors the server's tokenizer (admin.rs `tokenize_query`):
 * whitespace-split into terms, but a `"…"`-quoted span stays a single exact
 * term (spaces preserved). Terms are AND-matched server-side; the client uses
 * the same split to highlight every term in snippets and the chat window.
 */
export function tokenizeQuery(q: string): string[] {
	const terms: string[] = [];
	let cur = '';
	let inQuote = false;
	for (const c of q) {
		if (c === '"') {
			if (cur) terms.push(cur);
			cur = '';
			inQuote = !inQuote;
		} else if (/\s/.test(c) && !inQuote) {
			if (cur) terms.push(cur);
			cur = '';
		} else {
			cur += c;
		}
	}
	if (cur) terms.push(cur);
	return terms.slice(0, 8);
}

function escapeRegExp(s: string): string {
	return s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/**
 * Wrap every occurrence of any term in `<mark class="search-hit">`, touching
 * only text *outside* HTML tags so it never corrupts the markdown/highlight
 * markup it's layered over. Case-insensitive; longest terms first so a term
 * that is a prefix of another doesn't shadow it.
 */
export function highlightTerms(html: string, terms: string[]): string {
	const pat = terms
		.filter(Boolean)
		.sort((a, b) => b.length - a.length)
		.map(escapeRegExp)
		.join('|');
	if (!pat) return html;
	const re = new RegExp(`(${pat})`, 'gi');
	// Split into tags (`<…>`) and text runs; only the runs get highlighted.
	return html.replace(/<[^>]*>|[^<]+/g, (seg) =>
		seg.startsWith('<') ? seg : seg.replace(re, '<mark class="search-hit">$1</mark>')
	);
}
