<script lang="ts">
	import { useUsageAnalytics } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';
	import { Card, Heading, Stack, Text } from '@dorsk/tsumikit';
	import SegmentedControl from '$lib/components/molecules/SegmentedControl.svelte';
	import TokensOverTime from './TokensOverTime.svelte';
	import ModelBreakdown from './ModelBreakdown.svelte';
	import ActivityHeatmap from './ActivityHeatmap.svelte';
	import { RANGES, hasUsage, type Granularity } from './usage-analytics';

	let rangeKey = $state('30d');
	const range = $derived(RANGES.find((r) => r.key === rangeKey) ?? RANGES[2]);

	const q = useUsageAnalytics(() => range.days);
	const data = $derived(q.data);
	const show = $derived(hasUsage(data));

	const rangeOptions = RANGES.map((r) => ({ value: r.key, label: r.key }));
</script>

<Stack gap="var(--sp-3)">
	<div class="head">
		<Heading level={2} size="lg">{m.home_usage_title()}</Heading>
		<SegmentedControl
			value={rangeKey}
			options={rangeOptions}
			label={m.home_usage_range_label()}
			onchange={(v) => (rangeKey = v)}
		/>
	</div>

	{#if q.isLoading}
		<Card><Text tone="faint">{m.common_loading()}</Text></Card>
	{:else if !show}
		<Card><Text tone="faint">{m.home_usage_no_data()}</Text></Card>
	{:else if data}
		<Card>
			<Stack gap="var(--sp-2)">
				<Text weight="bold" size="sm">{m.home_usage_tokens_over_time()}</Text>
				<TokensOverTime
					buckets={data.buckets}
					days={range.days}
					granularity={data.granularity as Granularity}
				/>
			</Stack>
		</Card>

		<div class="split">
			<Card>
				<Stack gap="var(--sp-2)">
					<Text weight="bold" size="sm">{m.home_usage_models()}</Text>
					{#if data.models.length}
						<ModelBreakdown models={data.models} />
					{:else}
						<Text tone="faint" size="sm">{m.home_usage_no_data()}</Text>
					{/if}
				</Stack>
			</Card>
			<Card>
				<Stack gap="var(--sp-2)">
					<Text weight="bold" size="sm">{m.home_usage_heatmap()}</Text>
					<ActivityHeatmap cells={data.heatmap} />
				</Stack>
			</Card>
		</div>
	{/if}
</Stack>

<style>
	.head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: var(--sp-3);
		flex-wrap: wrap;
	}
	.split {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(18rem, 1fr));
		gap: var(--sp-3);
	}
</style>
