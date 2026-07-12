<script lang="ts">
	import { Icon, Select } from '@dorsk/tsumikit';
	import { DIMENSIONS, type Dimension } from '../../../routes/sessions/sessions.logic';

	// Toolbar picker for a session-list dimension (CCT-466 color · CCT-467 group),
	// mirroring ViewPicker: a native <select> overlaid on a styled square trigger.
	let {
		value,
		onchange,
		kind
	}: { value: Dimension; onchange: (v: Dimension) => void; kind: 'color' | 'group' } = $props();

	const noun = $derived(kind === 'color' ? 'Color' : 'Group');
	const current = $derived(DIMENSIONS.find((d) => d.value === value)?.label ?? 'None');
	const active = $derived(value !== 'none');
	const title = $derived(active ? `${noun} by: ${current}` : `${noun} by…`);
</script>

<div class="dim-picker btn-control btn-control-square" class:active {title} aria-label={title}>
	{#if kind === 'color'}
		<Icon size={18}>
			<circle cx="13.5" cy="6.5" r=".5" fill="currentColor" />
			<circle cx="17.5" cy="10.5" r=".5" fill="currentColor" />
			<circle cx="8.5" cy="7.5" r=".5" fill="currentColor" />
			<circle cx="6.5" cy="12.5" r=".5" fill="currentColor" />
			<path d="M12 2C6.5 2 2 6.5 2 12s4.5 10 10 10c.926 0 1.648-.746 1.648-1.688 0-.437-.18-.835-.437-1.125-.29-.289-.438-.652-.438-1.125a1.64 1.64 0 0 1 1.668-1.668h1.996c3.051 0 5.555-2.503 5.555-5.554C21.965 6.012 17.461 2 12 2Z" />
		</Icon>
	{:else}
		<Icon size={18}>
			<rect x="3" y="4" width="18" height="5" rx="1" />
			<rect x="3" y="13" width="18" height="5" rx="1" />
		</Icon>
	{/if}
	<Select
		variant="ghost"
		aria-label={`${noun} sessions by`}
		{value}
		onchange={(e) => onchange((e.currentTarget as HTMLSelectElement).value as Dimension)}
	>
		{#each DIMENSIONS as d (d.value)}
			<option value={d.value}>{noun} by: {d.label}</option>
		{/each}
	</Select>
</div>

<style>
	.dim-picker {
		position: relative;
		display: inline-flex;
		align-items: center;
		justify-content: center;
		flex: none;
		white-space: nowrap;
		cursor: pointer;
	}
	/* A set dimension gets the accent tint so an active color/group is visible at
	   a glance without opening the popup. */
	.dim-picker.active {
		color: var(--accent);
	}
</style>
