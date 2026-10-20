import { describe, expect, it } from 'vitest';
import {
	clampDockWidth,
	CONVERSATION_DOCK_WIDTH,
	DOCK_MAX_PX,
	DOCK_MIN_PX,
	maxDockWidth,
	resolveDocks,
	SPAWN_DOCK_WIDTH,
	storedDockWidth,
	STATS_DOCK_WIDTH
} from './dock';

const off = { enabled: false, side: 'right' as const };
const on = (side: 'left' | 'right') => ({ enabled: true, side });

describe('resolveDocks', () => {
	it('docks nothing below the wide breakpoint whatever is stored', () => {
		const r = resolveDocks({ spawn: on('left'), stats: on('right'), wide: false, veryWide: false });
		expect(r).toEqual({
			conversation: null,
			spawn: null,
			stats: null,
			stacked: false,
			left: null,
			right: null
		});
	});

	it('docks nothing in tiles mode, however wide the window and whatever is stored', () => {
		const inputs = {
			spawn: { ...on('left'), width: 420 },
			stats: { ...on('right'), width: 300 },
			wide: true,
			veryWide: true
		};
		expect(resolveDocks({ ...inputs, tiles: true })).toEqual({
			conversation: null,
			spawn: null,
			stats: null,
			stacked: false,
			left: null,
			right: null
		});
		expect(resolveDocks({ ...inputs, tiles: false })).toEqual(resolveDocks(inputs));
		expect(resolveDocks(inputs).spawn).toBe('left');
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

describe('resolveDocks with a docked conversation', () => {
	it('is off unless enabled, leaving the other panels exactly as before', () => {
		const base = { spawn: on('right'), stats: on('left'), wide: true, veryWide: true };
		expect(resolveDocks({ ...base, conversation: off })).toEqual(resolveDocks(base));
		expect(resolveDocks(base).conversation).toBeNull();
	});

	it('pins the conversation to the chosen edge at its default width', () => {
		for (const side of ['left', 'right'] as const) {
			const r = resolveDocks({ spawn: off, stats: off, conversation: on(side), wide: true, veryWide: false });
			expect(r.conversation).toBe(side);
			expect(r[side]).toBe(CONVERSATION_DOCK_WIDTH);
			expect(r[side === 'left' ? 'right' : 'left']).toBeNull();
		}
	});

	it('falls back to the drawer below the wide breakpoint and in tiles mode', () => {
		const inputs = { spawn: off, stats: off, conversation: on('right'), veryWide: true };
		expect(resolveDocks({ ...inputs, wide: false }).conversation).toBeNull();
		expect(resolveDocks({ ...inputs, wide: true, tiles: true }).conversation).toBeNull();
	});

	it('moves a panel asked for the same edge to the opposite one', () => {
		const r = resolveDocks({
			spawn: on('right'),
			stats: on('right'),
			conversation: on('right'),
			wide: true,
			veryWide: true
		});
		expect(r.conversation).toBe('right');
		expect(r.spawn).toBe('left');
		expect(r.stats).toBe('left');
		expect(r.stacked).toBe(true);
		expect(r.right).toBe(CONVERSATION_DOCK_WIDTH);
		expect(r.left).toBe(SPAWN_DOCK_WIDTH);
	});

	it('keeps a panel already on the other edge where it is', () => {
		const r = resolveDocks({
			spawn: off,
			stats: on('right'),
			conversation: on('left'),
			wide: true,
			veryWide: true
		});
		expect(r.conversation).toBe('left');
		expect(r.stats).toBe('right');
		expect(r.right).toBe(STATS_DOCK_WIDTH);
	});

	it('keeps only the conversation when there is no room for a second column', () => {
		const r = resolveDocks({
			spawn: on('left'),
			stats: on('left'),
			conversation: on('right'),
			wide: true,
			veryWide: false
		});
		expect(r.conversation).toBe('right');
		expect(r.spawn).toBeNull();
		expect(r.stats).toBeNull();
		expect(r.left).toBeNull();
	});

	it('uses the dragged conversation width, clamped like the others', () => {
		const at = (width: number) =>
			resolveDocks({
				spawn: off,
				stats: off,
				conversation: { ...on('right'), width },
				wide: true,
				veryWide: false
			}).right;
		expect(at(720)).toBe('720px');
		expect(at(5)).toBe(`${DOCK_MIN_PX}px`);
	});
});

describe('storedDockWidth', () => {
	it('treats a missing entry as unset instead of clamping 0 to the minimum', () => {
		expect(storedDockWidth(null)).toBeUndefined();
		expect(storedDockWidth('')).toBeUndefined();
		expect(storedDockWidth('480')).toBe(480);
		expect(storedDockWidth('abc')).toBeUndefined();
		expect(storedDockWidth('1')).toBe(DOCK_MIN_PX);
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

describe('maxDockWidth', () => {
	it('caps a dragged panel at a share of the viewport', () => {
		expect(maxDockWidth(2000)).toBe(1200);
	});

	it('keeps the floor on a viewport too narrow for the share', () => {
		expect(maxDockWidth(100)).toBe(DOCK_MIN_PX);
		expect(maxDockWidth(0)).toBe(DOCK_MIN_PX);
	});
});
