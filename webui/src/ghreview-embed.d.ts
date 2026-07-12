// Ambient type for the aliased gh-review root (CCT-610). Declared here rather
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
