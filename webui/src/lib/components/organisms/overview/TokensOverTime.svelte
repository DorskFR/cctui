<script lang="ts">
	import type { UsageBucket } from '@bindings/UsageBucket';
	import { compact } from '$lib/format';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import { Cluster, Text } from '@dorsk/tsumikit';
	import { fillBuckets, peakBucketTotal, type Granularity } from './usage-analytics';

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
	const peak = $derived(peakBucketTotal(filled));

	// A label under a bar every ~Nth slot so the axis stays readable at 24/30 bars.
	const labelEvery = $derived(granularity === 'hour' ? 6 : days <= 7 ? 1 : 5);
	const fmt = (ms: number) =>
		new Date(ms).toLocaleString(getLocale(), {
			...(granularity === 'hour' ? { hour: '2-digit' } : { month: 'short', day: 'numeric' })
		});
	const pct = (v: number) => `${(v / peak) * 100}%`;
</script>

<div class="chart">
	<div class="legend">
		<Cluster gap="var(--sp-3)">
			<span class="key"><i class="sw in"></i><Text size="xs" tone="muted">{m.home_usage_input()}</Text></span>
			<span class="key"><i class="sw out"></i><Text size="xs" tone="muted">{m.home_usage_output()}</Text></span>
			<span class="key"><i class="sw cache"></i><Text size="xs" tone="muted">{m.home_usage_cache()}</Text></span>
		</Cluster>
	</div>

	<div class="bars" role="list">
		{#each filled as b, i (b.ms)}
			<div
				class="col"
				role="listitem"
				title={`${fmt(b.ms)}\n↑${b.input}  ↓${b.output}  ⚡${b.cache_read}`}
			>
				<div class="stack">
					<div class="seg cache" style={`height:${pct(b.cache_read)}`}></div>
					<div class="seg out" style={`height:${pct(b.output)}`}></div>
					<div class="seg in" style={`height:${pct(b.input)}`}></div>
				</div>
				<div class="tick">
					{#if i % labelEvery === 0}<Text size="xs" tone="faint" numeric>{fmt(b.ms)}</Text>{/if}
				</div>
			</div>
		{/each}
	</div>
	<div class="peak"><Text size="xs" tone="faint">{m.home_usage_peak({ n: compact(peak) })}</Text></div>
</div>

<style>
	.chart {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.legend .key {
		display: inline-flex;
		align-items: center;
		gap: 0.35rem;
	}
	.sw {
		width: 0.7rem;
		height: 0.7rem;
		border-radius: 2px;
		display: inline-block;
	}
	.sw.in,
	.seg.in {
		background: var(--accent-dim);
	}
	.sw.out,
	.seg.out {
		background: var(--accent);
	}
	.sw.cache,
	.seg.cache {
		background: var(--warn);
	}
	.bars {
		display: flex;
		align-items: flex-end;
		gap: 2px;
		height: 8rem;
	}
	.col {
		flex: 1 1 0;
		min-width: 0;
		display: flex;
		flex-direction: column;
		height: 100%;
	}
	/* The stack grows from the baseline: segments are laid out bottom-up so the
	   whole bar height is the sum of the three metrics against the shared peak. */
	.stack {
		flex: 1;
		display: flex;
		flex-direction: column;
		justify-content: flex-end;
		min-height: 0;
	}
	.seg {
		width: 100%;
		border-radius: 1px 1px 0 0;
	}
	.col:hover .seg {
		filter: brightness(1.15);
	}
	.tick {
		height: 1rem;
		overflow: hidden;
		text-align: center;
		white-space: nowrap;
	}
	.peak {
		text-align: right;
	}
</style>
