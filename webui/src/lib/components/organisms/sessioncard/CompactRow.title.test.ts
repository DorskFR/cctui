import { describe, expect, it } from 'vitest';
import row from './CompactRow.svelte?raw';

const css = row.slice(row.indexOf('<style>'));

function rulesFor(selectorEnd: string): string[] {
	return [...css.matchAll(/([^{}]+)\{([^}]*)\}/g)]
		.filter(([, sel]) => sel.split(',').some((s) => s.trim().endsWith(selectorEnd)))
		.map(([, , body]) => body);
}

describe('list row title', () => {
	it('paints in the primary text tone', () => {
		expect(rulesFor('.title').some((b) => b.includes('color: var(--text);'))).toBe(true);
	});

	it('is never recoloured to a dimmer tier', () => {
		for (const body of rulesFor('.title')) {
			expect(body).not.toMatch(/color:\s*var\(--text-(muted|faint)\)/);
		}
	});
});
