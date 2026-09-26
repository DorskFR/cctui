// Ambient types for the aliased gh-review modules. Declared here rather
// than resolved to source so svelte-check never parses the sibling workspace
// and pulls in a second copy of svelte — see webui/vite.config.ts. This file
// has no imports/exports so it stays a global script (the decl is ambient).
declare module '$ghreview/Review.svelte' {
	const Review: import('svelte').Component<{
		baseUrl: string;
		token: string | null;
		account?: string | null;
		basePath?: string;
	}>;
	export default Review;
}

declare module '$ghreview/lib/markdown/highlight' {
	export const hljs: import('highlight.js').HLJSApi;
	export const LANG_ALIAS: Record<string, string>;
	export function escapeHtml(value: string): string;
	export function stripAnsi(value: string): string;
	export function looksLikeDiff(value: string): boolean;
	export function highlightDiff(value: string): string;
	export function highlightCode(rawCode: string, language: string): string;
}
