<script lang="ts">
	import { useAccounts, useAllMachines, useSessionStats, useTokenStats } from '$lib/queries';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import MetricTile from '$lib/components/molecules/MetricTile.svelte';
	import WindowsTable from '$lib/components/molecules/WindowsTable.svelte';
	import UsageAnalyticsSection from '$lib/components/organisms/overview/UsageAnalyticsSection.svelte';
	import { RANGES } from '$lib/components/organisms/overview/usage-analytics';
	import { Card, Heading, SegmentedControl, Stack, Text } from '@dorsk/tsumikit';
	import { buildMetricTiles, machinesOnline, type MetricKey } from './home.logic';

	const stats = useSessionStats();
	const tokens = useTokenStats();
	const machines = useAllMachines(() => true);
	const accounts = useAccounts();

	let rangeKey = $state('30d');
	const rangeOptions = RANGES.map((r) => ({ value: r.key, label: r.key }));

	const machineRows = $derived(machines.data ?? []);
	const scope = $derived(
		m.home_usage_scope({
			machines: machinesOnline(machineRows).total,
			accounts: (accounts.data ?? []).length
		})
	);

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
	<div class="head">
		<Heading level={1} size="xl">{m.home_usage_page_title()}</Heading>
		<Text size="xs" tone="muted">{scope}</Text>
		<div class="spacer"></div>
		<SegmentedControl
			bind:value={rangeKey}
			options={rangeOptions}
			size="sm"
			label={m.home_usage_range_label()}
		/>
	</div>

	<div class="tiles">
		{#each tiles as t (t.key)}
			<MetricTile
				value={num(t.value)}
				suffix={t.suffix}
				warn={t.warn}
				label={tileLabel(t.key, t.sub)}
			/>
		{/each}
	</div>

	<Card padding="none"><WindowsTable windows={tokens.data} /></Card>

	<UsageAnalyticsSection {rangeKey} />
</Stack>

<style>
	.head {
		display: flex;
		align-items: baseline;
		gap: var(--sp-3);
		flex-wrap: wrap;
	}
	.spacer {
		flex: 1;
	}
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
		gap: var(--sp-3);
	}
</style>
