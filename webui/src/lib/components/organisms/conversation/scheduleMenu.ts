import type { MenuItem } from '@dorsk/tsumikit';
import { schedulePresets } from './scheduleTimes';
import { m } from '$lib/paraglide/messages';

export interface ScheduleMenuOpts {
	now: number;
	canSchedule: boolean;
	onpreset: (at: Date) => void;
	oncustom: () => void;
	/** Omitted where there is no pending list to jump to (the spawn modal). */
	scheduledCount?: number;
	onlist?: () => void;
}

const hhmm = (d: Date) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

/** Items of a schedule split-button's menu: the presets, a custom time, and —
 *  when `onlist` is given — a jump to the pending list. */
export function scheduleMenuItems(o: ScheduleMenuOpts): MenuItem[] {
	const presets: MenuItem[] = schedulePresets(new Date(o.now)).map((p) => ({
		label:
			p.id === 'later'
				? m.composer_schedule_later_today({ time: hhmm(p.at) })
				: p.id === 'tomorrow'
					? m.composer_schedule_tomorrow({ time: hhmm(p.at) })
					: m.composer_schedule_monday({ time: hhmm(p.at) }),
		icon: 'clock',
		disabled: !o.canSchedule,
		onselect: () => o.onpreset(p.at)
	}));
	const items: MenuItem[] = [
		...presets,
		{
			label: m.composer_schedule_custom(),
			disabled: !o.canSchedule,
			onselect: o.oncustom
		}
	];
	if (o.onlist) {
		const count = o.scheduledCount ?? 0;
		items.push({
			label: m.composer_schedule_list({ count: String(count) }),
			disabled: count === 0,
			onselect: o.onlist
		});
	}
	return items;
}
