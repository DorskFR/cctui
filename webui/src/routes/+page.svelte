<script lang="ts">
	import { useMachineResources, useSessionStats, useTokenStats } from '$lib/queries';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import AccountStats from '$lib/stats/accounts/AccountStats.svelte';
	import SessionPeriodSummary from '$lib/components/organisms/overview/SessionPeriodSummary.svelte';
	import MetricTile from '$lib/components/molecules/MetricTile.svelte';
	import WindowsTable from '$lib/components/molecules/WindowsTable.svelte';
	import UsageAnalyticsSection from '$lib/components/organisms/overview/UsageAnalyticsSection.svelte';
	import { Card, Stack } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { buildMetricTiles, metricLabel } from './home.logic';

	const stats = useSessionStats();
	const tokens = useTokenStats();
	const machines = useMachineResources(() => true);

	const machineRows = $derived(machines.data ?? []);

	const num = (n: number) => n.toLocaleString(getLocale());
	const tiles = $derived(buildMetricTiles(stats.data, machineRows));
</script>

<Stack gap="var(--sp-5)">
	<PageHead title={m.home_usage_page_title()} />

	<div class="tiles" data-journey="tiles">
		{#each tiles as t (t.key)}
			<div data-journey="tile" data-journey-key={t.key}>
				<MetricTile
					value={num(t.value)}
					suffix={t.suffix}
					warn={t.warn}
					label={metricLabel(t.key, num(t.sub ?? 0))}
				/>
			</div>
		{/each}
	</div>

	<SessionPeriodSummary stats={stats.data} />

	<Card padding="none" data-journey="windows"><WindowsTable windows={tokens.data} /></Card>

	<Card><AccountStats size="full" /></Card>

	<div data-journey="analytics"><UsageAnalyticsSection /></div>
</Stack>

<style>
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
		gap: var(--sp-3);
	}
</style>
