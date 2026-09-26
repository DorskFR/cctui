/** Vite config for a cctui plugin's `web/` bundle: one ES module that leaves
 *  Svelte and Tsumikit to the host (`/plugin-runtime/*`) so the pane shares the
 *  page's runtime, context and reactivity. Component CSS is injected at mount
 *  (the host CSP allows inline styles). */
import { svelte } from '@sveltejs/vite-plugin-svelte';
import type { UserConfig } from 'vite';
import { PLUGIN_RUNTIME_PATHS } from './types';

export interface PluginBuildOptions {
	/** Module whose default export is the `CctuiPluginModule`. */
	entry: string;
	/** Where `index.js` lands; the folder `plugin.json#web` points at. */
	outDir?: string;
	/** Use the host's `/plugin-runtime` under a different prefix (tests). */
	runtimeBase?: string;
}

export function pluginRuntimePaths(runtimeBase = ''): Record<string, string> {
	if (!runtimeBase) return { ...PLUGIN_RUNTIME_PATHS };
	const out: Record<string, string> = {};
	for (const [spec, url] of Object.entries(PLUGIN_RUNTIME_PATHS)) out[spec] = `${runtimeBase}${url}`;
	return out;
}

export function cctuiPluginConfig(opts: PluginBuildOptions): UserConfig {
	const paths = pluginRuntimePaths(opts.runtimeBase);
	return {
		plugins: [svelte({ compilerOptions: { css: 'injected' }, emitCss: false })],
		build: {
			outDir: opts.outDir ?? 'dist/web',
			emptyOutDir: true,
			minify: false,
			lib: { entry: opts.entry, formats: ['es'], fileName: () => 'index.js' },
			rollupOptions: {
				external: (id) => id in paths,
				output: { paths }
			}
		}
	};
}
