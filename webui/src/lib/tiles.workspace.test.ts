// @vitest-environment happy-dom
import { describe, expect, it, vi } from 'vitest';
import { parseTileIds, TilesWorkspace, TILES_KEY } from './tiles.svelte';

function ws(over: Partial<{ max: number; saved: string }> = {}) {
	const store = new Map<string, string>();
	if (over.saved !== undefined) store.set(TILES_KEY, over.saved);
	const overCap = vi.fn();
	const urls: string[][] = [];
	const tiles = new TilesWorkspace({
		store: { get: (k) => store.get(k) ?? '', set: (k, v) => void store.set(k, v) },
		maxTiles: () => over.max ?? 4,
		onOverCap: overCap,
		writeUrl: (ids) => urls.push([...ids])
	});
	return { tiles, store, overCap, urls };
}

describe('parseTileIds', () => {
	it('reads a comma list, trimming blanks and duplicates', () => {
		expect(parseTileIds('a, b ,a,,c')).toEqual(['a', 'b', 'c']);
		expect(parseTileIds('')).toEqual([]);
		expect(parseTileIds(null)).toEqual([]);
	});
});

describe('TilesWorkspace', () => {
	it('adds, focuses and persists to storage and the URL', () => {
		const { tiles, store, urls } = ws();
		expect(tiles.add('a')).toBe(true);
		expect(tiles.add('b')).toBe(true);
		expect(tiles.ids).toEqual(['a', 'b']);
		expect(tiles.focused).toBe('b');
		expect(store.get(TILES_KEY)).toBe('a,b');
		expect(urls.at(-1)).toEqual(['a', 'b']);
	});

	it('holds a session at most once, focusing the tile it already has', () => {
		const { tiles } = ws();
		tiles.add('a');
		tiles.add('b');
		expect(tiles.add('a')).toBe(true);
		expect(tiles.ids).toEqual(['a', 'b']);
		expect(tiles.focused).toBe('a');
	});

	it('refuses to go over the cap and says so', () => {
		const { tiles, overCap } = ws({ max: 2 });
		tiles.add('a');
		tiles.add('b');
		expect(tiles.add('c')).toBe(false);
		expect(tiles.ids).toEqual(['a', 'b']);
		expect(overCap).toHaveBeenCalledWith(2);
		expect(tiles.full).toBe(true);
	});

	it('adds what fits of a batch and reports the cap once', () => {
		const { tiles, overCap } = ws({ max: 3 });
		expect(tiles.addMany(['a', 'b', 'c', 'd', 'e'])).toBe(3);
		expect(tiles.ids).toEqual(['a', 'b', 'c']);
		expect(overCap).toHaveBeenCalledTimes(1);
	});

	it('moves focus to a neighbour when the focused tile closes', () => {
		const { tiles } = ws();
		tiles.addMany(['a', 'b', 'c']);
		tiles.focus('b');
		tiles.remove('b');
		expect(tiles.ids).toEqual(['a', 'c']);
		expect(tiles.focused).toBe('c');
		tiles.remove('c');
		expect(tiles.focused).toBe('a');
		tiles.remove('a');
		expect(tiles.focused).toBe(null);
	});

	it('reorders within bounds', () => {
		const { tiles } = ws();
		tiles.addMany(['a', 'b', 'c']);
		tiles.move('c', -1);
		expect(tiles.ids).toEqual(['a', 'c', 'b']);
		tiles.move('a', -1);
		expect(tiles.ids, 'already first').toEqual(['a', 'c', 'b']);
		tiles.move('a', 99);
		expect(tiles.ids).toEqual(['c', 'b', 'a']);
	});

	it('shows only the maximized tile, and restores on close or toggle', () => {
		const { tiles } = ws();
		tiles.addMany(['a', 'b', 'c']);
		tiles.toggleMaximize('b');
		expect(tiles.visible).toEqual(['b']);
		expect(tiles.focused).toBe('b');
		tiles.toggleMaximize('b');
		expect(tiles.visible).toEqual(['a', 'b', 'c']);

		tiles.toggleMaximize('c');
		tiles.remove('c');
		expect(tiles.maximized).toBe(null);
		expect(tiles.visible).toEqual(['a', 'b']);
	});

	it('prefers a shared link over the persisted set', () => {
		const { tiles } = ws({ saved: 'x,y' });
		tiles.hydrate('a,b');
		expect(tiles.ids).toEqual(['a', 'b']);
		expect(tiles.focused).toBe('a');
	});

	it('falls back to the persisted set, clamped to the cap', () => {
		const { tiles } = ws({ saved: 'a,b,c,d,e', max: 3 });
		tiles.hydrate(null);
		expect(tiles.ids).toEqual(['a', 'b', 'c']);
	});

	it('clear empties everything', () => {
		const { tiles, store } = ws();
		tiles.addMany(['a', 'b']);
		tiles.toggleMaximize('a');
		tiles.clear();
		expect(tiles.ids).toEqual([]);
		expect(tiles.maximized).toBe(null);
		expect(tiles.focused).toBe(null);
		expect(store.get(TILES_KEY)).toBe('');
	});
});
