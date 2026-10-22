import { renderMarkdown } from '$lib/markdown';

/**
 * Twin of `fold` in `crates/cctui-server/src/search_index/text.rs`: the server
 * matches folded queries against folded transcript chunks, so both sides must
 * fold alike (`fixtures/parity/searchText.json`).
 */
export function foldSearchText(s: string): string {
	return s
		.normalize('NFKC')
		.replace(/[\u2018\u2019\u201A\u201B\u2032]/g, "'")
		.replace(/[\u201C\u201D\u201E\u201F\u2033]/g, '"')
		.replace(/[\u200B\u200C\u200D\u2060\uFEFF\u00AD]/g, '')
		.replace(/\u2022/g, ' ')
		.replace(/[\s\u0085]+/g, ' ')
		.trim();
}

const ENTITIES: Record<string, string> = {
	'&amp;': '&',
	'&lt;': '<',
	'&gt;': '>',
	'&quot;': '"',
	'&#39;': "'",
	'&#x27;': "'"
};

export function markdownToSearchText(src: string): string {
	const text = renderMarkdown(src, { sessionId: '-' })
		.replace(/<button\b[\s\S]*?<\/button>/g, '')
		.replace(/<\/t[dh]>/g, ' ')
		.replace(/<br \/>|<\/?(?:table|tr|div|pre)\b[^>]*>/g, '\n')
		.replace(/<[^>]*>/g, '')
		.replace(/&(?:amp|lt|gt|quot|#39|#x27);/g, (e) => ENTITIES[e] ?? e);
	return foldSearchText(text);
}
