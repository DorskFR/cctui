<script lang="ts">
	import { useAccounts, useAllMachines, useSessionStats, useTokenStats } from '$lib/queries';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import MetricTile from '$lib/components/molecules/MetricTile.svelte';
	import WindowsTable from '$lib/components/molecules/WindowsTable.svelte';
	import UsageAnalyticsSection from '$lib/components/organisms/overview/UsageAnalyticsSection.svelte';
	import { RANGES } from '$lib/components/organisms/overview/usage-analytics';
	import { Card, SegmentedControl, Stack } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { buildMetricTiles, machinesOnline, type MetricKey } from './home.logic';

	const stats = useSessionStats();
	const tokens = useTokenStats();
	const machines = useAllMachines(() => true);
	const accounts = useAccounts();

	let rangeKey = $state('30d');
	const rangeOptions = RANGES.map((r) => ({ value: r.key, label: r.key }));

	const machineRows = $derived(machines.data ?? []);

	const num = (n: number) => n.toLocaleString(getLocale());
	const tiles = $derived(buildMetricTiles(stats.data, machineRows));
	const tileLabel = (key: MetricKey, sub: number | undefined) =>
		key === 'live'
			? m.home_stat_live()
			: key === 'needs_input'
				? m.home_stat_needs_input()
				: key === 'machines'
					? m.home_stat_machines_online()
					: m.home_stat_total_sessions_archived({ n: num(sub ?? 0) });
</script>

<Stack gap="var(--sp-5)">
	<PageHead title={m.home_usage_page_title()}>
		<SegmentedControl bind:value={rangeKey} options={rangeOptions} label={m.home_usage_range_label()} />
	</PageHead>

	<div class="tiles" data-journey="tiles">
		{#each tiles as t (t.key)}
			<div data-journey="tile" data-journey-key={t.key}>
				<MetricTile
					value={num(t.value)}
					suffix={t.suffix}
					warn={t.warn}
					label={tileLabel(t.key, t.sub)}
				/>
			</div>
		{/each}
	</div>

	<Card padding="none" data-journey="windows"><WindowsTable windows={tokens.data} /></Card>

	<div data-journey="analytics"><UsageAnalyticsSection {rangeKey} /></div>
</Stack>

<style>
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
		gap: var(--sp-3);
	}
</style>
