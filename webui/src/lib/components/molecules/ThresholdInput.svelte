<script lang="ts">
	import type { AutoFollowupThreshold } from '@bindings/AutoFollowupThreshold';
	import type { ThresholdUnit } from '@bindings/ThresholdUnit';
	import { Input, Select, type SelectOption } from '@dorsk/tsumikit';
	import { clampThreshold, DEFAULT_THRESHOLD } from '#lib/autoFollowup.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		value,
		onchange,
		id,
		disabled = false,
		size
	}: {
		value: AutoFollowupThreshold;
		onchange: (next: AutoFollowupThreshold) => void;
		id?: string;
		disabled?: boolean;
		size?: 'sm';
	} = $props();

	const UNIT_DEFAULT: Record<ThresholdUnit, number> = {
		tokens: DEFAULT_THRESHOLD.value,
		percent: 80
	};
	const units: SelectOption[] = [
		{ value: 'tokens', label: m.autofollowup_unit_tokens() },
		{ value: 'percent', label: m.autofollowup_unit_percent() }
	];

	let invalid = $state(false);

	function onValue(e: Event) {
		const next = clampThreshold({
			unit: value.unit,
			value: (e.currentTarget as HTMLInputElement).valueAsNumber
		});
		invalid = next === null;
		if (next) onchange(next);
	}

	function onUnit(unit: string) {
		const u = unit as ThresholdUnit;
		if (u === value.unit) return;
		invalid = false;
		onchange({ unit: u, value: UNIT_DEFAULT[u] });
	}
</script>

<span class="threshold" data-testid="autofollowup-threshold">
	<Input
		{id}
		type="number"
		min="1"
		max={value.unit === 'percent' ? '100' : undefined}
		step={value.unit === 'percent' ? '1' : '1000'}
		value={value.value}
		{disabled}
		{invalid}
		{size}
		width="8rem"
		onchange={onValue}
		aria-label={m.autofollowup_threshold_label()}
	/>
	<Select
		options={units}
		{disabled}
		{size}
		width="auto"
		bind:value={() => value.unit, onUnit}
		aria-label={m.autofollowup_unit_label()}
	/>
</span>

<style>
	.threshold {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
</style>
