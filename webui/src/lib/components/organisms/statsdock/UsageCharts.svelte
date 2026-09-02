<script lang="ts">
	import { useUsageAnalytics } from '$lib/queries';
	import { Text } from '@dorsk/tsumikit';
	import SegmentedControl from '$lib/components/molecules/SegmentedControl.svelte';
	import TokensOverTime from '../overview/TokensOverTime.svelte';
	import ModelBreakdown from '../overview/ModelBreakdown.svelte';
	import ActivityHeatmap from '../overview/ActivityHeatmap.svelte';
	import { RANGES, hasUsage, type Granularity } from '../overview/usage-analytics';
	import { m } from '$lib/paraglide/messages';

	// The Overview's usage analytics (tokens over time, models, activity
	// heatmap) stacked for a narrow column. Defaults to the 7-day range so the
	// bars stay readable at panel width.
	let rangeKey = $state('7d');
	const range = $derived(RANGES.find((r) => r.key === rangeKey) ?? RANGES[1]);
	const q = useUsageAnalytics(() => range.days);
	const data = $derived(q.data);
	const rangeOptions = RANGES.map((r) => ({ value: r.key, label: r.key }));
</script>

<div class="charts">
	<SegmentedControl
		value={rangeKey}
		options={rangeOptions}
		label={m.home_usage_range_label()}
		onchange={(v) => (rangeKey = v)}
	/>
	{#if q.isLoading}
		<Text tone="faint" size="sm">{m.common_loading()}</Text>
	{:else if !data || !hasUsage(data)}
		<Text tone="faint" size="sm">{m.home_usage_no_data()}</Text>
	{:else}
		<div class="block">
			<Text weight="semibold" size="xs" tone="muted">{m.home_usage_tokens_over_time()}</Text>
			<TokensOverTime buckets={data.buckets} days={range.days} granularity={data.granularity as Granularity} />
		</div>
		<div class="block">
			<Text weight="semibold" size="xs" tone="muted">{m.home_usage_models()}</Text>
			{#if data.models.length}
				<ModelBreakdown models={data.models} />
			{:else}
				<Text tone="faint" size="sm">{m.home_usage_no_data()}</Text>
			{/if}
		</div>
		<div class="block">
			<Text weight="semibold" size="xs" tone="muted">{m.home_usage_heatmap()}</Text>
			<ActivityHeatmap cells={data.heatmap} />
		</div>
	{/if}
</div>

<style>
	.charts,
	.block {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.charts {
		gap: var(--sp-3);
	}
</style>
