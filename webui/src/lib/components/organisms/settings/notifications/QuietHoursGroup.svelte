<script lang="ts">
	import { Cluster, Input, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import {
		MAX_PUSH_PER_HOUR_LIMIT,
		browserTimeZone,
		clampMaxPushPerHour,
		clampQuietHours
	} from '#lib/notificationPrefs.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const prefs = $derived(settings.state.notifications);
	const quiet = $derived(prefs.quiet_hours);

	function toggleQuiet(on: boolean) {
		settings.setNotifications({
			quiet_hours: on ? { start: '22:00', end: '07:00', tz: browserTimeZone() } : null
		});
	}

	function patchQuiet(field: 'start' | 'end' | 'tz', value: string) {
		if (!quiet) return;
		const next = clampQuietHours({ ...quiet, [field]: value });
		if (next) settings.setNotifications({ quiet_hours: next });
	}
</script>

<SettingGroup>
	<SettingRow label={m.notif_set_quiet_label()} help={m.notif_set_quiet_help()} server>
		<Switch
			label={m.notif_set_quiet_label()}
			disabled={!prefs.enabled}
			bind:checked={() => quiet !== null, (v) => toggleQuiet(v)}
		/>
	</SettingRow>
	{#if quiet}
		<SettingRow label={m.notif_set_quiet_label()} wide selfLabelled>
			<Cluster gap="var(--sp-2)" align="center">
				<Input
					id="notif-quiet-start"
					type="time"
					size="sm"
					aria-label={m.notif_set_quiet_start()}
					value={quiet.start}
					onchange={(e) => patchQuiet('start', e.currentTarget.value)}
				/>
				<Input
					id="notif-quiet-end"
					type="time"
					size="sm"
					aria-label={m.notif_set_quiet_end()}
					value={quiet.end}
					onchange={(e) => patchQuiet('end', e.currentTarget.value)}
				/>
				<Input
					id="notif-quiet-tz"
					size="sm"
					grow
					aria-label={m.notif_set_quiet_tz()}
					placeholder={browserTimeZone()}
					value={quiet.tz}
					onchange={(e) => patchQuiet('tz', e.currentTarget.value)}
				/>
			</Cluster>
		</SettingRow>
	{/if}
	<SettingRow label={m.notif_set_cap_label()} help={m.notif_set_cap_help()} server>
		<Input
			type="number"
			size="sm"
			min="0"
			max={MAX_PUSH_PER_HOUR_LIMIT}
			aria-label={m.notif_set_cap_label()}
			value={String(prefs.max_push_per_hour)}
			onchange={(e) =>
				settings.setNotifications({
					max_push_per_hour: clampMaxPushPerHour(Number(e.currentTarget.value))
				})}
		/>
	</SettingRow>
</SettingGroup>
