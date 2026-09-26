import { describe, expect, it, vi } from 'vitest';
import { scheduleMenuItems } from './scheduleMenu';
import { schedulePresets } from './scheduleTimes';

const NOW = new Date(2026, 0, 7, 9, 0, 0).getTime();

function items(over: Partial<{ canSchedule: boolean; scheduledCount: number }> = {}) {
	const onpreset = vi.fn();
	const oncustom = vi.fn();
	const onlist = vi.fn();
	const list = scheduleMenuItems({
		now: NOW,
		canSchedule: over.canSchedule ?? true,
		scheduledCount: over.scheduledCount ?? 0,
		onpreset,
		oncustom,
		onlist
	});
	return { list, onpreset, oncustom, onlist };
}

describe('scheduleMenuItems', () => {
	it('lists every preset, then custom, then the pending list', () => {
		const presets = schedulePresets(new Date(NOW));
		const { list } = items();
		expect(list).toHaveLength(presets.length + 2);
		for (const it of list.slice(0, presets.length)) expect(it.icon).toBe('clock');
	});

	it('routes a preset pick to its time', () => {
		const presets = schedulePresets(new Date(NOW));
		const { list, onpreset } = items();
		list[0].onselect?.();
		expect(onpreset).toHaveBeenCalledWith(presets[0].at);
	});

	it('disables scheduling entries while nothing can be scheduled', () => {
		const { list } = items({ canSchedule: false, scheduledCount: 2 });
		const [, ...rest] = [...list].reverse();
		for (const it of rest) expect(it.disabled).toBe(true);
		expect(list[list.length - 1].disabled).toBe(false);
	});

	it('disables the pending list entry when nothing is scheduled', () => {
		const { list, onlist, oncustom } = items({ scheduledCount: 0 });
		const last = list[list.length - 1];
		expect(last.disabled).toBe(true);
		expect(last.label).toContain('0');
		last.onselect?.();
		expect(onlist).toHaveBeenCalledTimes(1);
		list[list.length - 2].onselect?.();
		expect(oncustom).toHaveBeenCalledTimes(1);
	});
});
