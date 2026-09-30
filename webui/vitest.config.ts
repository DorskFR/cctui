import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

// Tests run under node; files that need a DOM opt in with
// `// @vitest-environment happy-dom`.
export default defineConfig({
	plugins: [sveltekit()],
	resolve: {
		conditions: ['browser']
	},
	test: {
		environment: 'node',
		environmentOptions: {
			happyDOM: {
				settings: {
					disableJavaScriptFileLoading: true,
					disableCSSFileLoading: true,
					disableIframePageLoading: true
				}
			}
		},
		setupFiles: ['./vitest.setup.ts'],
		include: ['src/**/*.test.ts', 'scripts/**/*.test.mjs']
	}
});
