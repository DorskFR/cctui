import { describe, expect, it } from 'vitest';
import { clampDockWidth, DOCK_MAX_PX, DOCK_MIN_PX, resolveDocks, SPAWN_DOCK_WIDTH, STATS_DOCK_WIDTH } from './dock';

const off = { enabled: false, side: 'right' as const };
const on = (side: 'left' | 'right') => ({ enabled: true, side });

describe('resolveDocks', () => {
	it('docks nothing below the wide breakpoint whatever is stored', () => {
		const r = resolveDocks({ spawn: on('left'), stats: on('right'), wide: false, veryWide: false });
		expect(r).toEqual({ spawn: null, stats: null, stacked: false, left: null, right: null });
	});

	it('reserves each edge for the panel pinned to it', () => {
		const r = resolveDocks({ spawn: on('right'), stats: on('left'), wide: true, veryWide: true });
		expect(r.spawn).toBe('right');
		expect(r.stats).toBe('left');
		expect(r.stacked).toBe(false);
		expect(r.right).toBe(SPAWN_DOCK_WIDTH);
		expect(r.left).toBe(STATS_DOCK_WIDTH);
	});

	it('drops the stats panel when opposite edges need more room than there is', () => {
		const r = resolveDocks({ spawn: on('right'), stats: on('left'), wide: true, veryWide: false });
		expect(r.spawn).toBe('right');
		expect(r.stats).toBeNull();
		expect(r.left).toBeNull();
	});

	it('stacks both panels in one column when they share an edge', () => {
		const r = resolveDocks({ spawn: on('left'), stats: on('left'), wide: true, veryWide: false });
		expect(r.stacked).toBe(true);
		expect(r.left).toBe(SPAWN_DOCK_WIDTH);
		expect(r.right).toBeNull();
	});

	it('a lone stats panel reserves its own narrower width', () => {
		const r = resolveDocks({ spawn: off, stats: on('right'), wide: true, veryWide: false });
		expect(r.spawn).toBeNull();
		expect(r.stats).toBe('right');
		expect(r.right).toBe(STATS_DOCK_WIDTH);
	});

	it('a dragged width wins over the rem default, on the edge it was pinned to', () => {
		const r = resolveDocks({
			spawn: { ...on('right'), width: 420 },
			stats: { ...on('left'), width: 300 },
			wide: true,
			veryWide: true
		});
		expect(r.right).toBe('420px');
		expect(r.left).toBe('300px');
	});

	it('a stacked column takes the spawn width, ignoring the stats width', () => {
		const r = resolveDocks({
			spawn: { ...on('left'), width: 500 },
			stats: { ...on('left'), width: 300 },
			wide: true,
			veryWide: false
		});
		expect(r.stacked).toBe(true);
		expect(r.left).toBe('500px');
	});

	it('an out-of-range stored width is clamped, a junk one falls back to the default', () => {
		const r = resolveDocks({
			spawn: { ...on('right'), width: 10 },
			stats: { ...on('left'), width: Number.NaN },
			wide: true,
			veryWide: true
		});
		expect(r.right).toBe(`${DOCK_MIN_PX}px`);
		expect(r.left).toBe(STATS_DOCK_WIDTH);
	});
});

describe('clampDockWidth', () => {
	it('rounds and bounds a number, drops anything else', () => {
		expect(clampDockWidth(333.6)).toBe(334);
		expect(clampDockWidth(1)).toBe(DOCK_MIN_PX);
		expect(clampDockWidth(99999)).toBe(DOCK_MAX_PX);
		expect(clampDockWidth('400')).toBeUndefined();
		expect(clampDockWidth(undefined)).toBeUndefined();
		expect(clampDockWidth(Number.POSITIVE_INFINITY)).toBeUndefined();
	});
});
