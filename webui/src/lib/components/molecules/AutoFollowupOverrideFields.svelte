<script lang="ts">
	import type { AutoFollowupOverride } from '@bindings/AutoFollowupOverride';
	import { SegmentedControl, Switch, Text, type SegmentOption } from '@dorsk/tsumikit';
	import ThresholdInput from './ThresholdInput.svelte';
	import { enabledOf, pickOf, type OverridePick } from '#lib/autoFollowup.ts';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		value,
		onchange,
		windowKnown = true
	}: {
		value: AutoFollowupOverride;
		onchange: (next: AutoFollowupOverride) => void;
		windowKnown?: boolean;
	} = $props();

	const picks: SegmentOption[] = [
		{ value: 'inherit', label: m.autofollowup_pick_default() },
		{ value: 'on', label: m.autofollowup_pick_on() },
		{ value: 'off', label: m.autofollowup_pick_off() }
	];
</script>

<div class="fields" data-testid="autofollowup-override">
	<SegmentedControl
		options={picks}
		size="sm"
		label={m.autofollowup_override_label()}
		bind:value={
			() => pickOf(value.enabled),
			(v) => onchange({ ...value, enabled: enabledOf(v as OverridePick) })
		}
	/>
	<Switch
		size="sm"
		labelVisible
		label={m.autofollowup_custom_threshold()}
		bind:checked={
			() => value.threshold !== null,
			(on) => onchange({ ...value, threshold: on ? settings.autoFollowup.threshold : null })
		}
	/>
	{#if value.threshold}
		<ThresholdInput
			size="sm"
			value={value.threshold}
			onchange={(threshold) => onchange({ ...value, threshold })}
		/>
		{#if value.threshold.unit === 'percent' && !windowKnown}
			<Text size="xs" tone="faint" as="div">{m.autofollowup_percent_hint()}</Text>
		{/if}
	{/if}
</div>

<style>
	.fields {
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: var(--sp-2);
		min-width: 0;
	}
</style>
