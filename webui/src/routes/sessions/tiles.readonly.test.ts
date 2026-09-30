import { describe, expect, it } from 'vitest';
import tiles from './SessionTiles.svelte?raw';
import page from './+page.svelte?raw';

describe('opening tiles never costs a turn', () => {
	it('sends no message, reply or resume from the tiles view', () => {
		// Attaching is subscribe + history fetch. A view that opens sixteen
		// conversations at once must not wake sixteen agents.
		for (const forbidden of [
			'sendMessage',
			'sendBody',
			'.resume(',
			'sendText',
			'respondPermission',
			'interrupt('
		]) {
			expect(tiles, forbidden).not.toContain(forbidden);
		}
	});

	it('raises no spawn entry point of its own', () => {
		expect(tiles).not.toContain('SpawnModal');
		expect(tiles).not.toContain('Spawn');
	});
});

describe('tiles view mode', () => {
	it('is rendered in place on /sessions, beside the list', () => {
		expect(page).toContain('{#if sp.tiles}');
		expect(page).toContain('<SessionTiles');
		expect(page).toContain('<SessionSections');
	});

	it('takes the remaining viewport instead of growing the page', () => {
		const css = tiles.slice(tiles.indexOf('<style>'));
		expect(css).toMatch(/\.tiles \{[^}]*flex: 1/);
		expect(css).toMatch(/\.tiles \{[^}]*min-height: 0/);
		expect(page).toContain('holdFullBleed()');
	});

	it('sizes itself from the measured viewport, not from a CSS guess', () => {
		expect(tiles).toContain('bind:clientWidth={width}');
		expect(tiles).toContain('bind:clientHeight={height}');
		expect(tiles).toContain('tileLayout(shown.length, { width, height })');
		expect(tiles).toContain('repeat({layout.tracks}, 1fr)');
	});

	it('draws one hairline between neighbours and no radius', () => {
		const css = tiles.slice(tiles.indexOf('<style>'));
		expect(css).toContain('gap: 1px');
		expect(css).toContain('background: var(--border-strong)');
		expect(css).toContain('border-radius: 0');
	});

	it('gives a tile no close control and keeps maximize', () => {
		expect(tiles).not.toContain('onclose');
		expect(tiles).toContain('onmaximize');
	});

	it('adds no :global override', () => {
		expect(tiles).not.toContain(':global(');
	});
});
