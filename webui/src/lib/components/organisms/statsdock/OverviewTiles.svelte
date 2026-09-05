<script lang="ts">
	import { useAllMachines, useSessionStats } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';
	import { getLocale } from '$lib/paraglide/runtime';
	import MetricTile from '$lib/components/molecules/MetricTile.svelte';
	import { buildMetricTiles, type MetricKey } from '../../../../routes/home.logic';

	const stats = useSessionStats();
	const machines = useAllMachines(() => true);

	const num = (n: number) => n.toLocaleString(getLocale());
	const tiles = $derived(buildMetricTiles(stats.data, machines.data ?? []));
	const label = (key: MetricKey) =>
		key === 'live'
			? m.home_stat_live()
			: key === 'needs_input'
				? m.home_stat_needs_input()
				: key === 'machines'
					? m.home_stat_machines_online()
					: m.home_stat_total_sessions();
</script>

<div class="tiles">
	{#each tiles as t (t.key)}
		<MetricTile compact value={num(t.value)} suffix={t.suffix} warn={t.warn} label={label(t.key)} />
	{/each}
</div>

<style>
	.tiles {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
	}
</style>
