<script lang="ts">
	import type { UsageBucket } from '@bindings/UsageBucket';
	import { compact, usd } from '#lib/format.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { Text } from '@dorsk/tsumikit';
	import {
		axisLabel,
		axisTicks,
		bucketLabel,
		bucketTotal,
		fillBuckets,
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

	const labels = $derived(filled.map((b) => axisLabel(b.ms, granularity)));
	const widest = $derived(labels.reduce((a, b) => (b.length > a.length ? b : a), ''));
	let axisWidth = $state(0);
	let labelWidth = $state(0);
	const ticks = $derived(axisTicks(filled.length, axisWidth, labelWidth, granularity));
	const pct = (v: number) => `${(v / peakHeight) * 100}%`;
</script>

<div class="chart">
	<div class="bars" role="list">
		{#each filled as b, i (b.ms)}
			<div
				class="col"
				class:recent={i >= recent}
				role="listitem"
				title={`${bucketLabel(b.ms, granularity)}\n↑${b.input}  ↓${b.output}  ⚡${b.cache_read}  ${usd(b.cost_usd)}`}
			>
				<div class="bar" style={`height:${pct(bucketTotal(b))}`}></div>
			</div>
		{/each}
	</div>
	<div class="axis" bind:clientWidth={axisWidth}>
		<span class="probe" aria-hidden="true" bind:clientWidth={labelWidth}>
			<Text size="xs" numeric nowrap>{widest}</Text>
		</span>
		{#each ticks as t (t.index)}
			<span class="tick" style:left="{t.left}px" style:width="{labelWidth}px">
				<Text size="xs" tone="faint" numeric nowrap>{labels[t.index]}</Text>
			</span>
		{/each}
	</div>
	{#if peak}
		<Text size="xs" tone="faint"
			>{m.home_usage_peak({ n: compact(bucketTotal(peak)) })} ·
			<time datetime={new Date(peak.ms).toISOString()}>{bucketLabel(peak.ms, granularity)}</time>
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
	.axis {
		position: relative;
	}
	.probe {
		display: inline-block;
		visibility: hidden;
	}
	.tick {
		position: absolute;
		top: 0;
		text-align: center;
	}
</style>
