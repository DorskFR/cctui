<script lang="ts">
	import type { UsageBucket } from '@bindings/UsageBucket';
	import { compact, usd } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import { Text, Timestamp, formatTimestamp } from '@dorsk/tsumikit';
	import {
		bucketTotal,
		fillBuckets,
		isAxisTick,
		peakBucket,
		peakBucketTotal,
		rangeCost,
		recentFrom,
		type Granularity
	} from './usage-analytics';

	let {
		buckets,
		days,
		granularity
	}: {
		buckets: UsageBucket[];
		days: number;
		granularity: Granularity;
	} = $props();

	const filled = $derived(fillBuckets(buckets, days, granularity));
	const peakHeight = $derived(peakBucketTotal(filled));
	const peak = $derived(peakBucket(filled));
	const recent = $derived(recentFrom(filled.length));
	const cost = $derived(rangeCost(filled));

	const tickEvery = $derived(granularity === 'hour' ? 6 : days <= 7 ? 1 : 7);
	const tick = (ms: number) => formatTimestamp(ms, granularity === 'hour' ? 'time' : 'short-iso');
	const pct = (v: number) => `${(v / peakHeight) * 100}%`;
</script>

<div class="chart">
	<div class="bars" role="list">
		{#each filled as b, i (b.ms)}
			<div
				class="col"
				class:recent={i >= recent}
				role="listitem"
				title={`${formatTimestamp(b.ms, 'datetime')}\n↑${b.input}  ↓${b.output}  ⚡${b.cache_read}  ${usd(b.cost_usd)}`}
			>
				<div class="bar" style={`height:${pct(bucketTotal(b))}`}></div>
			</div>
		{/each}
	</div>
	<div class="axis">
		{#each filled as b, i (b.ms)}
			<div class="tick">
				{#if isAxisTick(i, filled.length, tickEvery)}
					<Text size="xs" tone="faint" numeric nowrap>{tick(b.ms)}</Text>
				{/if}
			</div>
		{/each}
	</div>
	{#if peak}
		<Text size="xs" tone="faint"
			>{m.home_usage_peak({ n: compact(bucketTotal(peak)) })} ·
			<Timestamp value={peak.ms} mode="datetime" tone="inherit" />
			{#if cost > 0}
				· {m.home_usage_range_cost({ usd: usd(cost) })}
			{/if}</Text
		>
	{/if}
</div>

<style>
	.chart {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.bars {
		display: flex;
		align-items: flex-end;
		gap: 3px;
		height: 6.25rem;
	}
	.col {
		flex: 1 1 0;
		min-width: 0;
		display: flex;
		flex-direction: column;
		justify-content: flex-end;
		height: 100%;
	}
	.bar {
		width: 100%;
		min-height: 1px;
		border-radius: 2px 2px 0 0;
		background: var(--accent-dim);
	}
	.col.recent .bar {
		background: var(--accent);
	}
	.col:hover .bar {
		filter: brightness(1.15);
	}
	/* The axis mirrors the bar track column-for-column so each label sits under
	   its own bar; overflow is visible so an edge label is not clipped. */
	.axis {
		display: flex;
		gap: 3px;
	}
	.tick {
		flex: 1 1 0;
		min-width: 0;
		display: flex;
		justify-content: center;
	}
</style>
