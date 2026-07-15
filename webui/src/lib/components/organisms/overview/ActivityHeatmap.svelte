<script lang="ts">
	import type { HeatmapCell } from '@bindings/HeatmapCell';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import { Text } from '@dorsk/tsumikit';
	import { buildHeatGrid } from './usage-analytics';

	let { cells }: { cells: HeatmapCell[] } = $props();

	const { grid, maxMessages } = $derived(buildHeatGrid(cells));

	// Localized single-letter weekday initials, Sunday-first to match dow 0..6.
	const dayLabels = $derived.by(() => {
		const fmt = new Intl.DateTimeFormat(getLocale(), { weekday: 'short' });
		return Array.from({ length: 7 }, (_, d) => fmt.format(new Date(2023, 0, 1 + d)));
	});
	const intensity = (msgs: number) => (maxMessages === 0 ? 0 : msgs / maxMessages);
</script>

<div class="heat">
	<div class="corner"></div>
	<div class="hours">
		{#each [0, 6, 12, 18] as h (h)}
			<Text size="xs" tone="faint" numeric>{String(h).padStart(2, '0')}</Text>
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
		gap: 2px var(--sp-2);
		align-items: center;
	}
	.corner {
		grid-column: 1;
	}
	.hours {
		display: flex;
		justify-content: space-between;
		padding: 0 0.1rem;
	}
	.daylabel {
		text-align: right;
	}
	.cells {
		display: grid;
		grid-template-columns: repeat(24, 1fr);
		gap: 2px;
	}
	/* Cell intensity blends the accent onto the elevated track by --i (0–1); a
	   faint floor keeps empty cells visible as the grid. */
	.cell {
		aspect-ratio: 1;
		border-radius: 2px;
		background: color-mix(in srgb, var(--accent) calc(var(--i) * 100%), var(--bg-elevated));
		min-height: 0.6rem;
	}
</style>
