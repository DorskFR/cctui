<script lang="ts">
	import type { HeatmapCell } from '@bindings/HeatmapCell';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import { Text } from '@dorsk/tsumikit';
	import { buildHeatGrid } from './usage-analytics';

	let { cells }: { cells: HeatmapCell[] } = $props();

	const { grid, maxMessages } = $derived(buildHeatGrid(cells));

	// Localized weekday labels, Sunday-first to match dow 0..6.
	const dayLabels = $derived.by(() => {
		const fmt = new Intl.DateTimeFormat(getLocale(), { weekday: 'short' });
		return Array.from({ length: 7 }, (_, d) => fmt.format(new Date(2023, 0, 1 + d)));
	});
	const hours = Array.from({ length: 24 }, (_, h) => h);
	const intensity = (msgs: number) => (maxMessages === 0 ? 0 : msgs / maxMessages);
</script>

<div class="heat">
	<div class="corner"></div>
	<div class="hours">
		{#each hours as h (h)}
			<div class="hour">
				{#if h % 3 === 0}
					<Text size="xs" tone="faint" numeric>{String(h).padStart(2, '0')}</Text>
				{/if}
			</div>
		{/each}
	</div>
	{#each grid as dowRow, dow (dow)}
		<div class="daylabel"><Text size="xs" tone="faint">{dayLabels[dow]}</Text></div>
		<div class="cells">
			{#each dowRow as cell, hour (hour)}
				<div
					class="cell"
					style={`--i:${intensity(cell.messages)}`}
					title={`${dayLabels[dow]} ${String(hour).padStart(2, '0')}:00\n${cell.messages} ${m.home_usage_messages()} · ↓${cell.output}`}
				></div>
			{/each}
		</div>
	{/each}
</div>

<style>
	.heat {
		display: grid;
		grid-template-columns: auto 1fr;
		gap: 3px var(--sp-2);
		align-items: center;
	}
	.corner {
		grid-column: 1;
	}
	.hours,
	.cells {
		display: grid;
		grid-template-columns: repeat(24, 1fr);
		gap: 3px;
	}
	.daylabel {
		text-align: right;
	}
	/* Intensity rides the cell's opacity so the accent stays the accent; an
	   underlay keeps an empty hour visible as part of the grid. */
	.cell {
		height: 0.875rem;
		border-radius: 2px;
		background: var(--bg);
	}
	.cell::after {
		content: '';
		display: block;
		height: 100%;
		border-radius: 2px;
		background: var(--accent);
		opacity: var(--i);
	}
</style>
