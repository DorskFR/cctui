import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import type { Plugin, Rollup } from 'vite';

/** Modules a runtime-loaded plugin must share with the host (one Svelte
 *  runtime per page, or context/reactivity split in two). Each becomes an
 *  extra entry of the SAME client build as the app, so rollup hands it the
 *  very module instances the app chunks use, and a stable `/plugin-runtime/
 *  <name>.js` re-export points plugins at the hashed entry. */
export const PLUGIN_RUNTIME_MODULES: Record<string, string> = {
	svelte: 'svelte',
	'svelte-internal-client': 'svelte/internal/client',
	'svelte-internal-disclose-version': 'svelte/internal/disclose-version',
	'svelte-store': 'svelte/store',
	tsumikit: '@dorsk/tsumikit'
};

/** Major of the host ↔ plugin contract; a plugin module declares the one it
 *  was written against and the host refuses any other. */
export const CCTUI_PLUGIN_API = 1;

const ENTRY_PREFIX = 'plugin-runtime/';

type Bundle = Rollup.OutputBundle;
type Chunk = Rollup.OutputChunk;

/** Kit only links the stylesheets of its own route graph; a plugin may pull a
 *  component the app never renders, so the shim links every stylesheet the
 *  entry's chunk graph carries. */
function stylesheets(bundle: Bundle, entry: Chunk): string[] {
	const css = new Set<string>();
	const seen = new Set<string>();
	const walk = (fileName: string) => {
		if (seen.has(fileName)) return;
		seen.add(fileName);
		const chunk = bundle[fileName];
		if (chunk?.type !== 'chunk') return;
		for (const f of chunk.viteMetadata?.importedCss ?? []) css.add(f);
		for (const f of chunk.imports) walk(f);
	};
	walk(entry.fileName);
	return [...css].sort();
}

function shim(entryFile: string, css: string[]): string {
	const links =
		css.length === 0
			? ''
			: `for (const href of ${JSON.stringify(css.map((f) => `/${f}`))}) {
	if (!document.querySelector(\`link[rel="stylesheet"][href="\${href}"]\`)) {
		const link = document.createElement('link');
		link.rel = 'stylesheet';
		link.href = href;
		document.head.append(link);
	}
}
`;
	return `${links}export * from '/${entryFile}';\n`;
}

export interface PluginRuntimeManifest {
	cctuiApi: number;
	svelte: string;
	tsumikit: string;
}

/** Version of the installed package, found by walking up from its entry
 *  point (a package need not export `./package.json`). */
function installedVersion(pkg: string): string {
	const require = createRequire(import.meta.url);
	let dir = dirname(require.resolve(pkg));
	for (;;) {
		const manifest = join(dir, 'package.json');
		if (existsSync(manifest)) {
			const parsed = JSON.parse(readFileSync(manifest, 'utf8'));
			if (parsed.name === pkg) return parsed.version;
		}
		const parent = dirname(dir);
		if (parent === dir) throw new Error(`cannot find package.json of ${pkg}`);
		dir = parent;
	}
}

export function runtimeManifest(): PluginRuntimeManifest {
	return {
		cctuiApi: CCTUI_PLUGIN_API,
		svelte: installedVersion('svelte'),
		tsumikit: installedVersion('@dorsk/tsumikit')
	};
}

export function pluginRuntime(): Plugin {
	let outDir = '';
	let client = false;
	return {
		name: 'cctui:plugin-runtime',
		config(_config, env) {
			client = env.command === 'build' && !env.isSsrBuild;
			if (!client) return;
			const input: Record<string, string> = {};
			for (const name of Object.keys(PLUGIN_RUNTIME_MODULES)) {
				input[`${ENTRY_PREFIX}${name}`] = `src/lib/plugin-runtime/${name}.ts`;
			}
			return { build: { rollupOptions: { input } } };
		},
		configResolved(config) {
			outDir = config.build.outDir;
		},
		writeBundle(_options, bundle) {
			if (!client) return;
			const dir = join(outDir, 'plugin-runtime');
			mkdirSync(dir, { recursive: true });
			for (const chunk of Object.values(bundle)) {
				if (chunk.type !== 'chunk' || !chunk.isEntry || !chunk.name.startsWith(ENTRY_PREFIX)) continue;
				const name = chunk.name.slice(ENTRY_PREFIX.length);
				writeFileSync(join(dir, `${name}.js`), shim(chunk.fileName, stylesheets(bundle, chunk)));
			}
			writeFileSync(join(dir, 'manifest.json'), `${JSON.stringify(runtimeManifest(), null, '\t')}\n`);
		}
	};
}
