import { describe, expect, it } from 'vitest';
import tiles from './SessionTiles.svelte?raw';
import page from './+page.svelte?raw';
import layout from '../+layout.svelte?raw';
import spawnDock from '../../lib/spawnDock.svelte.ts?raw';
import appCss from '../../lib/styles/app.css?raw';

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
		// A bare `1fr` row floors at the pane's min-content height and the page
		// grows a scrollbar; only minmax(0, …) lets the transcripts scroll inside.
		expect(tiles).toContain('minmax(0, 1fr)');
		expect(tiles).not.toMatch(/repeat\(\{[^}]*\}, 1fr\)/);
	});

	it('lets the layout decide full-bleed, never the page from inside it', () => {
		// The layout renders this page inside the branch full-bleed toggles, so a
		// page-raised flag unmounts its own setter and the two flip forever.
		expect(page).not.toContain('holdFullBleed');
		expect(layout).toContain('sessionsView.tiles');
		expect(layout).toContain("page.url.pathname.startsWith('/sessions')");
	});

	it('sizes itself from the measured viewport, not from a CSS guess', () => {
		expect(tiles).toContain('bind:clientWidth={width}');
		expect(tiles).toContain('bind:clientHeight={height}');
		expect(tiles).toContain('tileLayout(panes.length, { width, height })');
		expect(tiles).toContain('fittingPaneCount(shown.length, { width, height })');
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

	it('drops the docked panels from the one place the layout is resolved', () => {
		expect(spawnDock).toContain('tiles: sessionsView.tiles');
		expect(page).toContain('{#if sp.dockSide}');
		expect(page).not.toMatch(/dockSide[^\n]*tiles/);
	});

	it('takes the document off the scroll-container path while tiles are up', () => {
		expect(layout).toContain("root.dataset.tiles = 'on'");
		expect(layout).toMatch(/\.app\.tiles \{[^}]*height: 100dvh/);
		expect(layout).toMatch(/\.app\.tiles \{[^}]*overflow: clip/);
		expect(appCss).toMatch(/:root\[data-tiles="on"\][\s\S]*?overflow: clip/);
	});

	it('adds no :global override', () => {
		expect(tiles).not.toContain(':global(');
	});
});
