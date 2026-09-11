<script lang="ts">
	import type { PoolUsageView } from '@bindings/PoolUsageView';
	import type { PoolUsageWindow } from '@bindings/PoolUsageWindow';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import SoftLimit from '$lib/components/molecules/SoftLimit.svelte';
	import { countdown } from '$lib/components/molecules/usage-battery.logic';
	import {
		familyLabel,
		fmtDemand,
		poolWindowLabel,
		poolWindowPace,
		projectionState,
		weightsNote
	} from './pools-usage.logic';

	// A pool's aggregate gauges: one block per provider family (a pool never
	// mixes harnesses in a calculation, election is per family too), one bar
	// per window with the pool's weighted level, its pace glyph and, in the
	// tooltip, where the pool as a whole is heading. Read-only: the caps live
	// on the accounts.
	let { usage, compact = false }: { usage: PoolUsageView; compact?: boolean } = $props();

	const now = Date.now();

	function reading(w: PoolUsageWindow): string {
		const state = projectionState(w, now);
		const head =
			state.kind === 'wall'
				? m.pools_usage_projection_wall({ time: countdown(state.ms) })
				: state.kind === 'holds'
					? m.pools_usage_projection_holds()
					: m.pools_usage_projection_insufficient();
		const p = w.projection;
		if (!p) return head;
		return `${head} · ${m.pools_usage_projection_detail({
			demand: fmtDemand(p.demand_pct_per_hour),
			hours: Math.round(p.slope_hours)
		})}`;
	}
</script>

<div class="gauges" class:compact data-journey="pool-usage">
	{#each usage.families as fam (fam.family)}
		{@const weights = weightsNote(fam.members)}
		<section class="family">
			<header class="head">
				<Text as="span" size="xs" tone="faint">
					{m.pools_usage_family({ family: familyLabel(fam.family), n: fam.members.length })}
				</Text>
				{#if weights}
					<Text as="span" size="xs" tone="faint">{m.pools_usage_weights({ weights })}</Text>
				{/if}
			</header>
			{#if fam.windows.length === 0}
				<Text as="p" size="xs" tone="faint">{m.pools_usage_unknown()}</Text>
			{:else}
				<div class="rows">
					{#each fam.windows as w (w.key)}
						<SoftLimit
							label={poolWindowLabel(w)}
							utilization={w.level_pct}
							resets={w.next_reset_at}
							pace={poolWindowPace(w)}
							note={reading(w)}
						/>
					{/each}
				</div>
			{/if}
		</section>
	{/each}
	{#if !usage.failover}
		<Text as="p" size="xs" tone="warn">{m.pools_usage_failover_off()}</Text>
	{/if}
</div>

<style>
	.gauges {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		min-width: 0;
	}
	.family {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.head {
		display: flex;
		justify-content: space-between;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
	.rows {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.compact .head {
		justify-content: flex-start;
	}
</style>
