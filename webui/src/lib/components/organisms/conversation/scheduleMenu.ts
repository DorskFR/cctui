import { formatTimestamp, type MenuItem } from '@dorsk/tsumikit';
import { schedulePresets } from './scheduleTimes';
import { compact } from '$lib/format';
import { m } from '$lib/paraglide/messages';

/** The cold-cache entries, shown while Send starts a follow-up. */
export interface ColdSendOpts {
	burstTokens: number | null;
	canSend: boolean;
	onsendhere: () => void;
	/** Omitted where the follow-up modal is unavailable. */
	onoptions?: () => void;
}

export interface ScheduleMenuOpts {
	now: number;
	canSchedule: boolean;
	onpreset: (at: Date) => void;
	oncustom: () => void;
	/** Omitted where there is no pending list to jump to (the spawn modal). */
	scheduledCount?: number;
	onlist?: () => void;
	cold?: ColdSendOpts | null;
}

const hhmm = (d: Date) => formatTimestamp(d, 'time');

export function coldSendItems(o: ColdSendOpts): MenuItem[] {
	const items: MenuItem[] = [
		{
			id: 'send-here',
			label: o.burstTokens
				? m.composer_send_here_burst({ tokens: compact(o.burstTokens) })
				: m.composer_send_here(),
			icon: 'send',
			disabled: !o.canSend,
			onselect: o.onsendhere
		}
	];
	if (o.onoptions) {
		items.push({ id: 'followup-options', label: m.composer_followup_options(), icon: 'fork', onselect: o.onoptions });
	}
	return items;
}

/** Items of a schedule split-button's menu: the cold-cache entries when
 *  `cold` is given, the presets, a custom time, and — when `onlist` is given —
 *  a jump to the pending list. */
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
		...(o.cold ? coldSendItems(o.cold) : []),
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
