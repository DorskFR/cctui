<script lang="ts">
	// Compact segmented control: a horizontal row of OptionButtons that
	// act as a single-choice picker. Replaces <Select> dropdowns for tri-state /
	// enum knobs in the account settings editor so controls stay inline and never
	// overlap their row labels.
	import { OptionButton } from '@dorsk/tsumikit';

	interface Option {
		value: string;
		label: string;
	}

	let {
		value = $bindable(''),
		options,
		label,
		onchange
	}: {
		value?: string;
		options: Option[];
		label: string;
		onchange?: (v: string) => void;
	} = $props();

	function pick(v: string) {
		value = v;
		onchange?.(v);
	}
</script>

<div class="seg" role="group" aria-label={label}>
	{#each options as opt (opt.value)}
		<OptionButton row selected={value === opt.value} onclick={() => pick(opt.value)}>
			{opt.label}
		</OptionButton>
	{/each}
</div>

<style>
	.seg {
		display: inline-flex;
		gap: var(--sp-1);
		flex-wrap: wrap;
		justify-content: flex-end;
	}
</style>
