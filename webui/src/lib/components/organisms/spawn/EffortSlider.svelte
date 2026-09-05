<script lang="ts">
	// A discrete reasoning-effort slider. `levels[0]` is "" (the adapter's
	// default); the kit Slider snaps to each named level and its marks are the
	// tick labels, clickable.
	import { Field, Slider } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		id,
		levels,
		current,
		onset
	}: {
		id: string;
		levels: string[];
		current: string;
		onset: (v: string) => void;
	} = $props();

	const idx = $derived(Math.max(0, levels.indexOf(current)));
	const marks = $derived(levels.map((lv, i) => ({ value: i, label: lv || m.spawn_effort_default() })));
</script>

<Field label={m.spawn_effort_label()} for={id}>
	<div class="inset">
	<Slider
		{id}
		min={0}
		max={levels.length - 1}
		step={1}
		ticks
		{marks}
		style="--slider-accent: var(--c-blue)"
		bind:value={() => idx, (v) => onset(levels[Number(v)] ?? '')}
		aria-valuetext={levels[idx] || m.spawn_effort_default()}
	/>
	</div>
</Field>

<style>
	/* Mark labels centre on their tick, so the first and last would hang past
	   the track; half a label of inset keeps them inside the field. */
	.inset {
		padding-inline: 1.5rem;
	}
</style>
