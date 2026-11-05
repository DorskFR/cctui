// @vitest-environment happy-dom
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { THEMES } from '$lib/theme.svelte';

const KIT = 'node_modules/@dorsk/tsumikit/dist/styles';
const read = (p: string) => readFileSync(p, 'utf8');
const variables = read('src/lib/styles/variables.css');
const app = read('src/lib/styles/app.css');
const tokens = read(`${KIT}/tokens.css`);
const themes = read(`${KIT}/themes.css`);

type Decls = Map<string, string>;
function block(css: string, selector: string): Decls {
	const start = css.indexOf(`${selector} {`);
	expect(start, `${selector} block`).toBeGreaterThanOrEqual(0);
	const body = css.slice(start, css.indexOf('\n}', start));
	const out: Decls = new Map();
	for (const m of body.matchAll(/(--[\w-]+):\s*([^;]+);/g)) out.set(m[1], m[2].trim());
	return out;
}
function resolve(name: string, scopes: Decls[], depth = 0): string {
	expect(depth, `cycle resolving ${name}`).toBeLessThan(10);
	const raw = scopes.map((s) => s.get(name)).find((v) => v !== undefined);
	if (raw === undefined) throw new Error(`${name} is not declared`);
	return raw.replace(/var\((--[\w-]+)\)/g, (_, ref) => resolve(ref, scopes, depth + 1));
}

const PALETTE = [
	'bg', 'bg-elev', 'bg-elev-2', 'surface', 'border', 'border-strong', 'text', 'text-muted',
	'text-faint', 'accent', 'accent-ink', 'accent-dim', 'blue', 'amber', 'red', 'green', 'violet',
	'gold', 'teal'
].map((t) => `--c-${t}`);

describe('variables.css is a kit import, not a fork', () => {
	it('imports the kit tokens and themes and declares no palette of its own', () => {
		expect(variables).toContain('@import "@dorsk/tsumikit/styles/tokens.css";');
		expect(variables).toContain('@import "@dorsk/tsumikit/styles/themes.css";');
		expect(variables).not.toMatch(/\[data-theme=/);
		for (const t of PALETTE) expect(variables).not.toContain(`${t}:`);
		expect(variables).not.toContain('--box-');
		expect(variables).not.toContain('--control-height');
	});

	it('keeps only the app-specific tokens', () => {
		const root = block(variables, ':root');
		expect([...root.keys()].sort()).toEqual(
			[
				'--attention-bg-solid',
				'--c-brown',
				'--content-wide',
				'--count-bg',
				'--count-bg-active',
				'--count-fg',
				'--count-fg-active',
				'--divider',
				'--hit-min',
				'--role-context',
				'--role-peer',
				'--role-poll',
				'--role-queued',
				'--role-summary',
				'--role-thinking',
				'--row-selected-bg',
				'--status-attention',
				'--status-failed',
				'--status-idle',
				'--status-rail-w',
				'--status-working',
				'--surface-edge',
				'--term-bg'
			]
		);
	});

	it('raises the hit floor to the kit touch target on coarse pointers', () => {
		expect(variables).toMatch(/@media \(pointer: coarse\) \{\s*:root \{\s*--hit-min: var\(--touch-target\);/);
	});

	it.each(['dark', ...THEMES.map((t) => t.id).filter((id) => id !== 'dark')])(
		'%s resolves the status, count and selection tokens to colours',
		(id) => {
			const scopes = [block(variables, ':root'), ...(id === 'dark' ? [] : [block(themes, `[data-theme="${id}"]`)]), block(tokens, ':root')];
			for (const t of ['--status-working', '--status-attention', '--status-failed', '--status-idle', '--count-bg', '--count-fg', '--count-bg-active', '--count-fg-active', '--row-selected-bg', '--divider'])
				expect(resolve(t, scopes), `${id} ${t}`).not.toMatch(/var\(/);
		}
	);

	it('gets the box scale and touch target from the kit', () => {
		const root = block(tokens, ':root');
		for (const t of ['--box-xs', '--box-sm', '--box-md', '--box-lg', '--touch-target', '--control-height-compact', '--control-height-default', '--control-height-large'])
			expect(root.has(t), t).toBe(true);
	});
});

describe('every app theme resolves against the kit stylesheet', () => {
	const root = block(tokens, ':root');
	const ids = THEMES.map((t) => t.id).filter((id) => id !== 'dark');

	it.each(ids)('%s has a kit block with the full palette contract', (id) => {
		const theme = block(themes, `[data-theme="${id}"]`);
		for (const t of PALETTE) expect(theme.has(t), `${id} ${t}`).toBe(true);
		for (const t of ['--shadow-sm', '--shadow-md', '--shadow-lg', '--mach-bg-sl', '--mach-fg-sl', '--mach-border-sl'])
			expect(theme.has(t), `${id} ${t}`).toBe(true);
	});

	it.each(['dark', ...ids])('%s resolves Button tone=success ink and the app tokens to literals', (id) => {
		const scopes = id === 'dark' ? [root] : [block(themes, `[data-theme="${id}"]`), root];
		for (const t of ['--text-on-success', '--text-on-accent', '--accent', '--ok'])
			expect(resolve(t, scopes)).toMatch(/^(#[0-9a-f]{3,8}|rgba?\(|hsla?\()/i);
		const appScopes = [block(variables, ':root'), ...scopes];
		expect(resolve('--role-thinking', appScopes)).not.toMatch(/var\(/);
		expect(resolve('--attention-bg-solid', appScopes)).not.toMatch(/var\(/);
	});
});

describe('app.css does not re-declare kit classes', () => {
	it('imports the kit reset and utilities', () => {
		expect(app).toContain('@import "@dorsk/tsumikit/styles/reset.css";');
		expect(app).toContain('@import "@dorsk/tsumikit/styles/utilities.css";');
	});
	it('loads the bundled Inter first and the kit overrides after the kit', () => {
		const imports = [...app.matchAll(/^@import "([^"]+)";/gm)].map((m) => m[1]);
		expect(imports[0]).toBe('@dorsk/tsumikit/styles/font.css');
		expect(imports.at(-1)).toBe('./kit-overrides.css');
	});
	it('declares the .num and .hit-target utilities', () => {
		expect(app).toMatch(/^\.num \{[^}]*font-family: var\(--font-numeric\);[^}]*tabular-nums/m);
		expect(app).toMatch(/^\.hit-target:not\(\[data-tsu\], \.btn, \.pop-trigger\)::before \{[^}]*inset: min\(0px, calc\(\(100% - var\(--hit-min\)\) \/ 2\)\)/m);
	});
	it('keeps .hit-target off kit controls, which own their pseudo-elements', () => {
		const rules = [...app.matchAll(/^([^\n{]*\.hit-target[^\n{]*)\{/gm)].map((m) => m[1].trim());
		expect(rules.length).toBeGreaterThan(0);
		for (const sel of rules) expect(sel).toMatch(/^\.hit-target:not\(\[data-tsu\], \.btn, \.pop-trigger\)/);
		const before = app.slice(app.indexOf('.hit-target:not([data-tsu], .btn, .pop-trigger)::before'));
		expect(before.slice(0, before.indexOf('}'))).toContain('background: none;');
	});
	it.each(['.btn-icon', '.btn-icon-inline', '.btn-control', '.btn-control-square', '.card', '.card-tap', '.container', '.stack', '.row', '.sr-only', '.divider', '.empty'])(
		'%s is not declared',
		(cls) => {
			expect(app).not.toMatch(new RegExp(`^${cls.replace('.', '\\.')}\\s*[{,]`, 'm'));
		}
	);
	it('renamed the empty placeholder away from the kit EmptyState class', () => {
		expect(app).toMatch(/^\.placeholder \{/m);
	});
});

function walk(dir: string, out: string[] = []): string[] {
	for (const name of readdirSync(dir)) {
		const p = join(dir, name);
		if (statSync(p).isDirectory()) walk(p, out);
		else if (/\.(svelte|css|js)$/.test(name)) out.push(p);
	}
	return out;
}

describe('every token the kit reads resolves from the imported stylesheets', () => {
	const declared = new Set<string>();
	for (const css of [tokens, themes, variables])
		for (const m of css.matchAll(/(--[\w-]+):/g)) declared.add(m[1]);
	const kitFiles = walk(`node_modules/@dorsk/tsumikit/dist/components`);
	// Set by the component itself (style:--x / inline style / setProperty).
	const local = new Set<string>();
	for (const f of kitFiles) {
		const src = readFileSync(f, 'utf8');
		for (const m of src.matchAll(/(?:style:|style="|`|;\s*|^\s*)(--[\w-]+)\s*[:=]/gm)) local.add(m[1]);
		for (const m of src.matchAll(/setProperty\(\s*['"](--[\w-]+)['"]/g)) local.add(m[1]);
	}
	// Consumer-provided geometry; the kit reads them without a fallback on
	// purpose so the shell owner can size the docks.
	const consumer = new Set(['--dock-left-w', '--dock-right-w']);

	it('leaves no fallback-less var() unresolved', () => {
		const missing = new Set<string>();
		for (const f of kitFiles)
			for (const m of readFileSync(f, 'utf8').matchAll(/var\((--[\w-]+)\)/g)) {
				const name = m[1];
				if (name.endsWith('-') || declared.has(name) || local.has(name) || consumer.has(name)) continue;
				missing.add(name);
			}
		expect([...missing]).toEqual([]);
	});

	it.each(['--control-height', '--control-height-compact', '--control-height-default', '--control-height-large', '--box-xs', '--box-sm', '--box-md', '--box-lg', '--touch-target', '--text-on-success'])(
		'%s resolves to a literal length or colour',
		(name) => {
			expect(resolve(name, [block(tokens, ':root')])).toMatch(/^(\d|#|rgb|hsl)/);
		}
	);
});

describe('the journey overlay is themed from the palette, not the library fallbacks', () => {
	const overlay = read('node_modules/@dorsk/journey/dist/runtime/overlay.js');
	const fallbacks = new Map<string, string>();
	for (const m of overlay.matchAll(/var\((--journey-[\w-]+),\s*([^()]*(?:\([^()]*\)[^()]*)*)\)/g))
		fallbacks.set(m[1], m[2].trim());
	const host = block(app, 'journey-overlay');
	const root = block(tokens, ':root');

	it('reads at least the accent, surface and text tokens', () => {
		for (const t of ['--journey-accent', '--journey-surface', '--journey-text'])
			expect(fallbacks.has(t), t).toBe(true);
	});

	it('maps every token the package reads, and nothing it does not', () => {
		expect([...host.keys()].sort()).toEqual([...fallbacks.keys()].sort());
	});

	// Resolved values cannot be compared to the fallbacks: --r-sm is 6px and so
	// is the library's. Sourcing from a palette var() is the check that holds.
	// A keyword, not a colour or a measure: the palette has no analogue to source it from.
	const KEYWORD_TOKENS = new Set(['--journey-banner-align']);

	it('sources every token from the palette rather than a literal of its own', () => {
		for (const [name, value] of host)
			if (!KEYWORD_TOKENS.has(name)) expect(value, name).toMatch(/var\(--/);
	});

	it.each(['dark', ...THEMES.map((t) => t.id).filter((id) => id !== 'dark')])(
		'%s resolves every token to a literal',
		(id) => {
			const scopes = [host, ...(id === 'dark' ? [] : [block(themes, `[data-theme="${id}"]`)]), root];
			for (const name of fallbacks.keys())
				expect(resolve(name, scopes), `${id} ${name}`).not.toMatch(/var\(/);
		}
	);
});

describe('kit-overrides.css', () => {
	const overrides = read('src/lib/styles/kit-overrides.css');

	it('marks every rule with what it asks of the kit', () => {
		const rules = [...overrides.matchAll(/^(?:@media[^{]+|[^\s/*@}][^{]*)\{/gm)].map((m) => m.index);
		for (const at of rules) {
			const before = overrides.slice(0, at).trimEnd();
			expect(before.endsWith('*/') && /\/\* tsumikit: [^*]+\*\/$/.test(before), overrides.slice(at, at + 40)).toBe(true);
		}
		expect(rules.length).toBeGreaterThan(0);
	});

	it('grows the kit tap-area slab to 44 px on coarse pointers', () => {
		const coarse = overrides.slice(overrides.indexOf('@media (pointer: coarse) {'));
		const body = coarse.slice(0, coarse.indexOf('\n}'));
		expect(body).toContain('--touch-target-compact: 44px;');
		expect(body).toContain('--touch-slab: min(-8px, calc((100% - var(--touch-target-compact)) / 2));');
		expect(block(tokens, ':root').get('--touch-slab')).toContain('var(--touch-target-compact)');
		const kitReads = walk('node_modules/@dorsk/tsumikit/dist/components')
			.map((f) => readFileSync(f, 'utf8'))
			.join('\n');
		expect(kitReads).toContain('var(--touch-slab)');
	});

	it('renders markdown prose in the body text tier', () => {
		expect(overrides).toMatch(/:root \{\s*--md-text: var\(--c-text\);/);
	});

	it('serves the hyphen from a bundled face without the tabular-figure substitution', () => {
		const face = overrides.slice(overrides.indexOf('@font-face {'), overrides.indexOf('}', overrides.indexOf('@font-face {')));
		expect(face).toContain('font-family: "Inter";');
		expect(face).toContain('unicode-range: U+002D;');
		const src = face.match(/url\("\.\/([^"]+)"\)/)?.[1];
		expect(src).toBe('fonts/inter-hyphen.woff2');
		const size = statSync(`src/lib/styles/${src}`).size;
		expect(size).toBeGreaterThan(0);
		expect(size).toBeLessThan(4096);
		expect(app.indexOf('./kit-overrides.css')).toBeGreaterThan(app.indexOf('@dorsk/tsumikit/styles/font.css'));
	});

	it('rounds every font-size token to a whole pixel', () => {
		const root = block(overrides, ':root');
		for (const step of ['2xs', 'xs', 'sm', 'base', 'md', 'lg', 'xl', '2xl', '3xl'])
			expect(root.get(`--fs-${step}`), step).toMatch(/^round\(.+, 1px\)$/);
	});

	it('floors the scale at 12 px and tops it at 28 px', () => {
		const root = block(overrides, ':root');
		expect(root.get('--fs-2xs')).toBe(root.get('--fs-xs'));
		expect(root.get('--fs-3xl')).toBe(root.get('--fs-2xl'));
	});
});

describe('deprecated spacing steps', () => {
	// --sp-1-5 (6 px) and --sp-5 (20 px) sit off the 4/8/12/16/24/32/48 scale.
	// Lower the baseline as call sites move; never raise it.
	const BASELINE = 0;

	it('are used no more than the recorded baseline', () => {
		const sources = (dir: string, out: string[] = []): string[] => {
			for (const name of readdirSync(dir)) {
				const p = join(dir, name);
				if (statSync(p).isDirectory()) {
					if (name !== 'paraglide' && name !== 'styles') sources(p, out);
				} else if (/\.(svelte|css|ts)$/.test(name) && !name.endsWith('.test.ts')) out.push(p);
			}
			return out;
		};
		const hits: string[] = [];
		for (const f of sources('src'))
			for (const m of readFileSync(f, 'utf8').matchAll(/var\(--sp-(?:1-5|5)\b[^)]*\)/g)) hits.push(`${f}: ${m[0]}`);
		expect(hits.length, hits.join('\n')).toBeLessThanOrEqual(BASELINE);
	});
});
