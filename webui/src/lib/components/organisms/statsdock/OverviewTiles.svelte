<script lang="ts">
	import { useMachineResources, useSessionStats } from '$lib/queries';
	import { getLocale } from '$lib/paraglide/runtime';
	import MetricTile from '$lib/components/molecules/MetricTile.svelte';
	import { buildMetricTiles, metricLabel } from '../../../../routes/home.logic';

	const stats = useSessionStats();
	const machines = useMachineResources(() => true);

	const num = (n: number) => n.toLocaleString(getLocale());
	const tiles = $derived(buildMetricTiles(stats.data, machines.data ?? []));
</script>

<div class="tiles">
	{#each tiles as t (t.key)}
		<MetricTile compact value={num(t.value)} suffix={t.suffix} warn={t.warn} label={metricLabel(t.key)} />
	{/each}
</div>

<style>
	.tiles {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
	}
</style>
