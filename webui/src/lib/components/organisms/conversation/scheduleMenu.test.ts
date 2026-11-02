import { describe, expect, it, vi } from 'vitest';
import { coldSendItems, scheduleMenuItems, type ColdSendOpts } from './scheduleMenu';
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

	it('drops the pending-list entry where there is no list (the spawn modal)', () => {
		const presets = schedulePresets(new Date(NOW));
		const oncustom = vi.fn();
		const list = scheduleMenuItems({ now: NOW, canSchedule: true, onpreset: vi.fn(), oncustom });
		expect(list).toHaveLength(presets.length + 1);
		list[list.length - 1].onselect?.();
		expect(oncustom).toHaveBeenCalledTimes(1);
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

describe('cold Send menu entries', () => {
	function cold(over: Partial<ColdSendOpts> = {}): ColdSendOpts {
		return { burstTokens: 93_000, canSend: true, onsendhere: vi.fn(), onoptions: vi.fn(), ...over };
	}

	it('are absent while the cache is warm', () => {
		const warm = scheduleMenuItems({ now: NOW, canSchedule: true, onpreset: vi.fn(), oncustom: vi.fn() });
		const unset = scheduleMenuItems({ now: NOW, canSchedule: true, onpreset: vi.fn(), oncustom: vi.fn(), cold: null });
		for (const list of [warm, unset]) {
			expect(list.some((it) => it.id === 'send-here' || it.id === 'followup-options')).toBe(false);
		}
	});

	it('come first while cold: Send here, then Follow-up with options, then the schedule items', () => {
		const presets = schedulePresets(new Date(NOW));
		const list = scheduleMenuItems({
			now: NOW,
			canSchedule: true,
			onpreset: vi.fn(),
			oncustom: vi.fn(),
			cold: cold()
		});
		expect(list.slice(0, 2).map((it) => it.id)).toEqual(['send-here', 'followup-options']);
		expect(list).toHaveLength(presets.length + 3);
		expect(list[2].icon).toBe('clock');
	});

	it('names the cache rebuild cost on Send here', () => {
		const [here] = coldSendItems(cold());
		expect(here.label).toContain('93');
		const [bare] = coldSendItems(cold({ burstTokens: null }));
		expect(bare.label).not.toContain('~');
	});

	it('route each entry to its action', () => {
		const o = cold();
		const [here, options] = coldSendItems(o);
		here.onselect?.();
		options.onselect?.();
		expect(o.onsendhere).toHaveBeenCalledTimes(1);
		expect(o.onoptions).toHaveBeenCalledTimes(1);
	});

	it('disable Send here with nothing to send, and drop the options entry without a modal', () => {
		const list = coldSendItems(cold({ canSend: false, onoptions: undefined }));
		expect(list.map((it) => it.id)).toEqual(['send-here']);
		expect(list[0].disabled).toBe(true);
	});
});
