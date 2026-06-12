<script lang="ts">
	import { useAccountUsage } from '$lib/queries';
	import { relativeTime } from '$lib/format';

	// Per-account subscription-usage shown as horizontal bars (CCT-345), styled
	// after the menubar "Agent Usage" popover: one row per window (5h / 7d) with a
	// label, a colored fill, and a right-aligned percent + reset hint. Reuses the
	// same lazy/slow-refresh fetch as UsageChip; renders nothing for providers
	// without a usage API (Codex) or while there's no data.
	let {
		id,
		provider,
		enabled = true
	}: {
		id: string;
		provider: string;
		enabled?: boolean;
	} = $props();

	const active = $derived(enabled && provider === 'anthropic');
	const q = useAccountUsage(
		() => id,
		() => active
	);

	const usage = $derived($q.data?.usage ?? null);

	type Win = { utilization?: number | null; resets_at?: string | null } | null | undefined;
	function row(label: string, w: Win) {
		const u = w?.utilization;
		if (u === null || u === undefined) return null;
		const pct = Math.max(0, Math.min(100, Math.round(u)));
		const tone = pct >= 90 ? 'hot' : pct >= 70 ? 'warm' : 'ok';
		return { label, pct, tone, resets: w?.resets_at ?? null };
	}

	const bars = $derived(
		[
			row('5h', usage?.five_hour),
			row('7d', usage?.seven_day),
			row('7d Opus', usage?.seven_day_opus),
			row('7d Sonnet', usage?.seven_day_sonnet)
		].filter((r): r is NonNullable<typeof r> => r !== null)
	);
</script>

{#if active && bars.length}
	<div class="bars">
		{#each bars as b (b.label)}
			<div class="bar-row">
				<span class="bar-label">{b.label}</span>
				<span class="bar-pct" class:warm={b.tone === 'warm'} class:hot={b.tone === 'hot'}>
					{b.pct}%{#if b.resets}<span class="bar-reset"> · resets {relativeTime(b.resets)}</span>{/if}
				</span>
				<div class="bar-track">
					<div
						class="bar-fill"
						class:warm={b.tone === 'warm'}
						class:hot={b.tone === 'hot'}
						style="width: {b.pct}%"
					></div>
				</div>
			</div>
		{/each}
	</div>
{:else if active && $q.isLoading}
	<span class="spin"></span>
{:else}
	<span class="faint">No usage data</span>
{/if}

<style>
	.bars {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
	.bar-row {
		display: grid;
		grid-template-columns: auto 1fr;
		align-items: baseline;
		column-gap: var(--sp-2);
	}
	.bar-label {
		font-size: var(--fs-xs);
		color: var(--text-muted);
		font-variant-numeric: tabular-nums;
	}
	.bar-pct {
		justify-self: end;
		font-size: var(--fs-xs);
		font-variant-numeric: tabular-nums;
		color: var(--c-green, #3fb950);
	}
	.bar-pct.warm {
		color: var(--warn, #d29922);
	}
	.bar-pct.hot {
		color: var(--danger, #f85149);
	}
	.bar-reset {
		color: var(--text-faint);
	}
	.bar-track {
		grid-column: 1 / -1;
		margin-top: 0.25rem;
		height: 6px;
		border-radius: 999px;
		background: var(--bg-elevated-2);
		overflow: hidden;
	}
	.bar-fill {
		height: 100%;
		border-radius: 999px;
		background: var(--c-green, #3fb950);
		transition: width 0.2s var(--ease);
	}
	.bar-fill.warm {
		background: var(--warn, #d29922);
	}
	.bar-fill.hot {
		background: var(--danger, #f85149);
	}
</style>
