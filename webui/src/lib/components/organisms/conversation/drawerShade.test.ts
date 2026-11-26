import { describe, expect, it } from 'vitest';
import header from './DrawerHeader.svelte?raw';
import toolbar from './DrawerToolbar.svelte?raw';
import filterMenu from './FilterMenu.svelte?raw';
import meta from './HeaderMeta.svelte?raw';

const css = (src: string) => src.slice(src.indexOf('<style>'));

describe('drawer chrome separates by shade, not hairlines', () => {
	it('draws no borders in the header, toolbar, filter menu or details', () => {
		for (const [name, src] of Object.entries({ header, toolbar, filterMenu, meta })) {
			expect(css(src), name).not.toMatch(/border(-top|-bottom|-left|-right)?:\s*1px/);
		}
	});

	it('paints the toolbar on the header surface so the two read as one band', () => {
		expect(css(header)).toContain('background: var(--bg-elevated)');
		expect(css(toolbar)).toContain('background: var(--bg-elevated)');
	});
});
