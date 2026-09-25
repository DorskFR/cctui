import type { MenuItem } from '@dorsk/tsumikit';
import { schedulePresets } from './scheduleTimes';
import { m } from '$lib/paraglide/messages';

export interface ScheduleMenuOpts {
	now: number;
	canSchedule: boolean;
	scheduledCount: number;
	onpreset: (at: Date) => void;
	oncustom: () => void;
	onlist: () => void;
}

const hhmm = (d: Date) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

/** Items of the Send split-button's schedule menu: the presets, a custom
 *  time, and a jump to the pending list. */
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
	return [
		...presets,
		{
			label: m.composer_schedule_custom(),
			disabled: !o.canSchedule,
			onselect: o.oncustom
		},
		{
			label: m.composer_schedule_list({ count: String(o.scheduledCount) }),
			disabled: o.scheduledCount === 0,
			onselect: o.onlist
		}
	];
}
