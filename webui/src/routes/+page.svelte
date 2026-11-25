<script lang="ts">
	import { useMachineResources, useSessionStats, useSessions, useTokenStats } from '#lib/queries/index.ts';
	import { getLocale } from '#lib/paraglide/runtime.js';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import SessionPeriodSummary from '#lib/components/organisms/overview/SessionPeriodSummary.svelte';
	import MetricTile from '#lib/components/molecules/MetricTile.svelte';
	import WindowsTable from '#lib/components/molecules/WindowsTable.svelte';
	import UsageAnalyticsSection from '#lib/components/organisms/overview/UsageAnalyticsSection.svelte';
	import { Card, Cluster, Link, Stack, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import { buildMetricTiles, needsInputHref, type MetricKey } from './home.logic';

	const stats = useSessionStats();
	const tokens = useTokenStats();
	const machines = useMachineResources(() => true);
	const sessions = useSessions(() => false);

	const machineRows = $derived(machines.data ?? []);

	const needs = $derived(stats.data?.needs_input ?? 0);
	const openHref = $derived(needsInputHref(sessions.data?.sessions ?? []));
	const num = (n: number) => n.toLocaleString(getLocale());
	const tiles = $derived(
		buildMetricTiles(stats.data, machineRows).filter(
			(t) => t.key !== 'machines' || hasFeature('machine_spawn')
		)
	);
	const tileLabel = (key: MetricKey, sub: number | undefined) =>
		key === 'live'
			? m.home_stat_working()
			: key === 'needs_input'
				? m.home_stat_needs_input()
				: key === 'machines'
					? m.home_stat_machines_online()
					: m.home_stat_total_sessions_archived({ n: num(sub ?? 0) });
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
					label={tileLabel(t.key, t.sub)}
					href={t.href}
				/>
			</div>
		{/each}
	</div>

	{#if needs > 0}
		<Cluster gap="var(--sp-2)" align="center">
			<Text tone="warn">{m.home_needs_input_line({ count: needs })}</Text>
			<Link href={openHref}>{m.home_needs_input_open()}</Link>
		</Cluster>
	{/if}

	<SessionPeriodSummary stats={stats.data} />

	<Card padding="none" data-journey="windows"><WindowsTable windows={tokens.data} /></Card>

	<div data-journey="analytics"><UsageAnalyticsSection /></div>
</Stack>

<style>
	.tiles {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(10rem, 1fr));
		gap: var(--sp-3);
	}
</style>
