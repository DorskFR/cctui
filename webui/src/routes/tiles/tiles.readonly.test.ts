import { describe, expect, it } from 'vitest';
import page from './+page.svelte?raw';
import grid from './TilesGrid.svelte?raw';
import toolbar from './TilesToolbar.svelte?raw';
import workspace from '$lib/tiles.svelte.ts?raw';

const sources = { page, grid, toolbar, workspace };

describe('opening tiles never costs a turn', () => {
	it('sends no message, reply or resume from anywhere in the view', () => {
		// Attaching is subscribe + history fetch. A view that opens nine
		// conversations at once must not wake nine agents.
		for (const [name, src] of Object.entries(sources)) {
			for (const forbidden of [
				'sendMessage',
				'sendBody',
				'.resume(',
				'sendText',
				'respondPermission',
				'interrupt('
			]) {
				expect(src, `${name} → ${forbidden}`).not.toContain(forbidden);
			}
		}
	});

	it('spawns only from the explicit + New button', () => {
		expect(page).toContain('<SpawnModal');
		expect(page.match(/SpawnModal/g) ?? []).toHaveLength(2);
		expect(grid).not.toContain('Spawn');
	});
});

describe('tiles grid visuals', () => {
	it('draws one hairline between neighbours and no radius', () => {
		const css = grid.slice(grid.indexOf('<style>'));
		expect(css).toContain('gap: 1px');
		expect(css).toContain('background: var(--border-strong)');
		expect(css).toContain('border-radius: 0');
		expect(css).toMatch(/\.tile\.focused \{[^}]*outline: 1px solid var\(--accent\)/);
		expect(css).toMatch(/\.tile\.focused \{[^}]*outline-offset: -1px/);
		expect(css).not.toContain('padding: var(--sp');
	});

	it('places each tile from the pure layout, not from a CSS guess', () => {
		expect(grid).toContain('repeat({layout.tracks}, 1fr)');
		expect(grid).toContain('{place?.start ?? 1} / span {place?.span ?? 1}');
	});

	it('adds no :global override', () => {
		for (const [name, src] of Object.entries(sources)) {
			expect(src, name).not.toContain(':global(');
		}
	});
});
