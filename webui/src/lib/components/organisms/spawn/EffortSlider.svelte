<script lang="ts">
	// A discrete reasoning-effort slider, extracted from SpawnModal. `levels[0]`
	// is "" (the adapter's default); the track snaps to each named level and the
	// active label is highlighted. Per-adapter level sets are passed in.
	import Field from '$lib/components/molecules/Field.svelte';

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
</script>

<Field label="Effort" for={id}>
	<input
		{id}
		class="slider"
		type="range"
		min="0"
		max={levels.length - 1}
		step="1"
		value={idx}
		oninput={(e) => onset(levels[Number((e.currentTarget as HTMLInputElement).value)])}
	/>
	<div class="ticks">
		{#each levels as lv, i (lv)}
			<button
				type="button"
				class="tick"
				class:on={i === idx}
				onclick={() => onset(lv)}>{lv || 'default'}</button
			>
		{/each}
	</div>
</Field>

<style>
	.slider {
		width: 100%;
		accent-color: var(--c-blue);
		margin: 2px 0;
	}
	.ticks {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-1);
	}
	.tick {
		flex: 1;
		padding: 2px 0;
		background: none;
		border: none;
		text-align: center;
		font-size: var(--fs-xs);
		color: var(--text-muted);
		cursor: pointer;
	}
	.tick.on {
		color: var(--c-blue);
		font-weight: var(--fw-medium);
	}
</style>
