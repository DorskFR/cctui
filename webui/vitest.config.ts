import { sveltekit } from '@sveltejs/kit/vite';
import { globSync, readFileSync } from 'node:fs';
import { defineConfig } from 'vitest/config';
import { kit } from './vite.config';

const include = ['src/**/*.test.ts', 'scripts/**/*.test.mjs'];

// Without isolation, happy-dom globals and vi.mock registrations leak into the
// next file, so only node files that mock nothing share a worker.
const isolated = include
	.flatMap((pattern) => globSync(pattern))
	.filter((file) => {
		const src = readFileSync(file, 'utf8');
		return (
			/@vitest-environment\s+happy-dom/.test(src) ||
			/\bvi\.(mock|doMock|stubGlobal|stubEnv|resetModules)\b/.test(src)
		);
	});

// Tests run under node; files that need a DOM opt in with
// `// @vitest-environment happy-dom`.
export default defineConfig({
	plugins: [sveltekit(kit)],
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
		// Node 25+ defines its own global localStorage, which shadows happy-dom's.
		execArgv: ['--no-webstorage'],
		setupFiles: ['./vitest.setup.ts'],
		projects: [
			{
				extends: true,
				test: { name: 'isolated', include: isolated }
			},
			{
				extends: true,
				test: {
					name: 'shared',
					include,
					exclude: isolated,
					pool: 'threads',
					isolate: process.env.VITEST_ISOLATE === '1'
				}
			}
		]
	}
});
