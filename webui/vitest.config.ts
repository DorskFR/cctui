import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

// Tests run under node; files that need a DOM opt in with
// `// @vitest-environment happy-dom`.
export default defineConfig({
	plugins: [sveltekit()],
	resolve: {
		conditions: ['browser'],
		alias: {
			$ghreview: new URL('../ghreview-ui/src', import.meta.url).pathname
		}
	},
	test: {
		environment: 'node',
		include: ['src/**/*.test.ts', 'scripts/**/*.test.mjs']
	}
});
