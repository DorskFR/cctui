import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

// Unit tests (CCT-338): the sveltekit() plugin resolves the `$lib` / `@bindings`
// aliases so the conversation formatting helpers import cleanly under vitest.
export default defineConfig({
	plugins: [sveltekit()],
	resolve: {
		conditions: ['browser']
	},
	test: {
		environment: 'happy-dom',
		include: ['src/**/*.test.ts']
	}
});
