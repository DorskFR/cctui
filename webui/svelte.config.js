import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
	preprocess: vitePreprocess(),
	kit: {
		// SPA / app mode — no SSR, no SEO. All routes resolved client-side and
		// served via the index.html fallback.
		adapter: adapter({ fallback: 'index.html' }),
		alias: {
			'@bindings': 'src/lib/bindings'
		},
		// Without a poll SvelteKit never re-reads _app/version.json, so a tab that
		// outlived a deploy keeps importing chunks the new build does not have.
		version: { pollInterval: 60_000 }
	}
};

export default config;
