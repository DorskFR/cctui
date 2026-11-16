<script lang="ts">
	import { Cluster, Input, Select } from '@dorsk/tsumikit';
	import type { NotificationStage } from '@bindings/NotificationStage';
	import { AFTER_MINUTES_MAX, clampAfterMinutes } from '$lib/notificationPrefs';
	import { m } from '$lib/paraglide/messages';
	import { channelLabel, kindLabel } from './labels';

	let {
		kind,
		channel,
		stage,
		disabled = false,
		onchange
	}: {
		kind: string;
		channel: string;
		stage: NotificationStage;
		disabled?: boolean;
		onchange: (stage: NotificationStage) => void;
	} = $props();

	const AFTER_DEFAULT = 10;
	const minutes = $derived(stage.mode === 'after' ? stage.minutes : AFTER_DEFAULT);
	const names = $derived({ kind: kindLabel(kind), channel: channelLabel(channel) });
	const options = $derived([
		{ value: 'off', label: m.notif_stage_off() },
		{ value: 'now', label: m.notif_stage_now() },
		{ value: 'after', label: m.notif_stage_delayed({ minutes }) }
	]);

	function setMode(mode: string) {
		if (mode === 'after') onchange({ mode: 'after', minutes });
		else if (mode === 'off' || mode === 'now') onchange({ mode });
	}
</script>

<Cluster gap="var(--sp-1)" align="center" wrap={false} data-stage-cell="{kind}:{channel}">
	<Select
		id="notif-stage-{kind}-{channel}"
		size="sm"
		width="auto"
		aria-label={m.notif_stage_cell_label(names)}
		value={stage.mode}
		{options}
		{disabled}
		onchange={(e) => setMode(e.currentTarget.value)}
	/>
	{#if stage.mode === 'after'}
		<Input
			id="notif-minutes-{kind}-{channel}"
			size="sm"
			type="number"
			min="1"
			max={AFTER_MINUTES_MAX}
			width="4.5rem"
			aria-label={m.notif_stage_minutes_label(names)}
			value={String(stage.minutes)}
			{disabled}
			onchange={(e) =>
				onchange({ mode: 'after', minutes: clampAfterMinutes(Number(e.currentTarget.value)) })}
		/>
	{/if}
</Cluster>

