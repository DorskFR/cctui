// @vitest-environment happy-dom
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

// A raw colour in a component cannot follow the theme, and a `var(--x, raw)`
// fallback silently becomes the rendered value when `--x` is not a kit token —
// so the fallback, not the token, is what ships.
const ALLOWED = new Set([
	// A mask gradient: the colour is an alpha channel, not a surface.
	'src/lib/components/organisms/conversation/ThinkingBubble.svelte'
]);

function walk(dir: string, out: string[] = []): string[] {
	for (const e of readdirSync(dir)) {
		const p = join(dir, e);
		if (statSync(p).isDirectory()) walk(p, out);
		else if (e.endsWith('.svelte')) out.push(p);
	}
	return out;
}

describe('component colours come from tokens', () => {
	it('no component carries a raw rgba()/hex value', () => {
		const root = join(process.cwd(), 'src');
		const offenders = walk(root)
			.map((f) => f.replace(`${root}`, 'src'))
			.filter((f) => !ALLOWED.has(f))
			.filter((f) => /rgba\(|#[0-9a-fA-F]{3,6}\b/.test(readFileSync(join(process.cwd(), f), 'utf8')));
		expect(offenders).toEqual([]);
	});
});

const KIT = join(process.cwd(), 'node_modules/@dorsk/tsumikit/dist/styles');
const kitTokens = readFileSync(join(KIT, 'tokens.css'), 'utf8');
const kitThemes = readFileSync(join(KIT, 'themes.css'), 'utf8');
const overrides = readFileSync(join(process.cwd(), 'src/lib/styles/kit-overrides.css'), 'utf8');

function decls(css: string, selector: string): Map<string, string> {
	const out = new Map<string, string>();
	const start = css.indexOf(`${selector} {`);
	if (start < 0) return out;
	const body = css.slice(start, css.indexOf('\n}', start));
	for (const m of body.matchAll(/(--[\w-]+):\s*([^;]+);/g)) out.set(m[1], m[2].trim());
	return out;
}

const THEME_IDS = [
	'dark',
	...new Set([...kitThemes.matchAll(/^\[data-theme="([\w-]+)"\] \{/gm)].map((m) => m[1]))
];

function palette(id: string): (name: string) => string {
	const scopes = [
		decls(overrides, `[data-theme="${id}"]`),
		...(id === 'dark' ? [] : [decls(kitThemes, `[data-theme="${id}"]`)]),
		decls(kitTokens, ':root')
	];
	return (name) => {
		const v = scopes.map((s) => s.get(`--c-${name}`)).find((x) => x !== undefined);
		if (!v || !/^#[0-9a-f]{6}$/i.test(v)) throw new Error(`${id} --c-${name} is not a #rrggbb literal: ${v}`);
		return v;
	};
}

function rgb(hex: string): [number, number, number] {
	return [1, 3, 5].map((i) => Number.parseInt(hex.slice(i, i + 2), 16) / 255) as [number, number, number];
}

function hue(hex: string): number {
	const [r, g, b] = rgb(hex);
	const max = Math.max(r, g, b);
	const d = max - Math.min(r, g, b);
	if (d === 0) return Number.NaN;
	const h = max === r ? ((g - b) / d) % 6 : max === g ? (b - r) / d + 2 : (r - g) / d + 4;
	return (h * 60 + 360) % 360;
}

function hueDistance(a: string, b: string): number {
	const d = Math.abs(hue(a) - hue(b)) % 360;
	return Math.min(d, 360 - d);
}

function luminance(hex: string): number {
	const [r, g, b] = rgb(hex).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
	return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: string, b: string): number {
	const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
	return (hi + 0.05) / (lo + 0.05);
}

describe('theme contract', () => {
	it('covers every kit theme', () => {
		expect(THEME_IDS.length).toBeGreaterThanOrEqual(30);
	});

	it.each(THEME_IDS)('%s keeps the accent at least 30° of hue from every status colour', (id) => {
		const c = palette(id);
		for (const status of ['green', 'amber', 'red'])
			expect(hueDistance(c('accent'), c(status)), `${id} accent vs ${status}`).toBeGreaterThanOrEqual(30);
	});

	it.each(THEME_IDS)('%s muted text reaches 4.5:1 on the base', (id) => {
		const c = palette(id);
		expect(contrast(c('text-muted'), c('bg'))).toBeGreaterThanOrEqual(4.5);
	});

	// Dark themes whose muted tier is still too close to body text; fixed in the kit, not here.
	const TIER_GAPS = new Set(['mocha', 'nord', 'gruvbox', 'onedark', 'slate', 'moss', 'zenburn']);
	const darkThemes = THEME_IDS.filter((id) => luminance(palette(id)('bg')) < 0.2);

	it.each(darkThemes.filter((id) => !TIER_GAPS.has(id)))(
		'%s separates body text from muted text by at least 1.3x luminance',
		(id) => {
			const c = palette(id);
			expect((luminance(c('text')) + 0.05) / (luminance(c('text-muted')) + 0.05)).toBeGreaterThanOrEqual(1.3);
		}
	);

	it('lists only tier gaps that still exist', () => {
		const closed = [...TIER_GAPS].filter((id) => {
			const c = palette(id);
			return (luminance(c('text')) + 0.05) / (luminance(c('text-muted')) + 0.05) >= 1.3;
		});
		expect(closed).toEqual([]);
	});
});
