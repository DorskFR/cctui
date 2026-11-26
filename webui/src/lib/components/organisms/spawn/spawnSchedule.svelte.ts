import type { MenuItem } from '@dorsk/tsumikit';
import { scheduleMenuItems } from '../conversation/scheduleMenu';
import { parseCustom, toLocalInput } from '../conversation/scheduleTimes';
import { toasts } from '#lib/toast.svelte.ts';
import { m } from '#lib/paraglide/messages.js';
import type { SpawnForm } from './spawnForm.svelte';

export class SpawnSchedule {
	customOpen = $state(false);
	customValue = $state('');
	repeatOpen = $state(false);
	// The presets are relative to now, so a form left open past a cutoff
	// (e.g. "later today") must not keep offering a time that has passed.
	#now = $state(Date.now());
	#sf: SpawnForm;

	constructor(sf: SpawnForm) {
		this.#sf = sf;
		$effect(() => {
			const t = setInterval(() => (this.#now = Date.now()), 60_000);
			return () => clearInterval(t);
		});
	}

	items = $derived.by<MenuItem[]>(() => {
		const sf = this.#sf;
		const items: MenuItem[] = [];
		if (sf.canSchedule) {
			items.push(
				...scheduleMenuItems({
					now: this.#now,
					canSchedule: sf.draftValid && !sf.busy,
					onpreset: (at) => void sf.scheduleLaunch(at),
					oncustom: () => {
						this.customValue = toLocalInput(new Date(Date.now() + 3_600_000));
						this.customOpen = true;
					}
				})
			);
			if (sf.canRepeat) {
				items.push({
					id: 'repeat',
					label: m.spawn_schedule_repeat(),
					icon: 'calendar',
					disabled: !sf.draftValid || sf.busy,
					onselect: () => {
						this.repeatOpen = true;
					}
				});
			}
		}
		items.push({
			id: 'auto-archive',
			label: m.spawn_auto_archive(),
			icon: 'archive',
			pressed: sf.form.auto_archive,
			keepOpen: true,
			attrs: { title: m.spawn_auto_archive_hint(), 'data-journey': 'auto-archive' },
			onselect: () => {
				sf.form.auto_archive = !sf.form.auto_archive;
			}
		});
		return items;
	});

	confirmCustom = () => {
		const at = parseCustom(this.customValue, new Date());
		if (!at) {
			toasts.error(m.composer_schedule_custom_invalid());
			return;
		}
		this.customOpen = false;
		void this.#sf.scheduleLaunch(at);
	};
}
