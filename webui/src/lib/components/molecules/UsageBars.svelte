<script lang="ts">
	import { useAccountUsage } from '$lib/queries';
	import { relativeFuture } from '$lib/format';
	import Text from '$lib/components/atoms/Text.svelte';

	// Severity breakpoints on window utilization (%). Below WARN → green ("ok"),
	// WARN–HOT → amber ("warm"), at/above HOT → red ("hot"). Named so the bar
	// colour and the percent text stay in lockstep (CCT-324).
	const WARN_PCT = 70;
	const HOT_PCT = 90;

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
		const tone = pct >= HOT_PCT ? 'hot' : pct >= WARN_PCT ? 'warm' : 'ok';
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
				<Text size="xs" tone="muted" class="bar-label">{b.label}</Text>
				<Text size="xs" class={`bar-pct${b.tone === 'warm' ? ' warm' : ''}${b.tone === 'hot' ? ' hot' : ''}`}>
					{b.pct}%{#if b.resets}<Text tone="faint" class="bar-reset"> · resets {relativeFuture(b.resets)}</Text>{/if}
				</Text>
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
	<Text tone="faint">No usage data</Text>
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
	/* These elements are rendered by the Text atom (which owns their size/tone),
	   so the residual layout + tone-colour chrome must be :global to reach them. */
	:global(.bar-label) {
		font-variant-numeric: tabular-nums;
	}
	:global(.bar-pct) {
		justify-self: end;
		font-variant-numeric: tabular-nums;
		color: var(--ok, #3fb950);
	}
	:global(.bar-pct.warm) {
		color: var(--warn, #d29922);
	}
	:global(.bar-pct.hot) {
		color: var(--danger, #f85149);
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
		background: var(--ok, #3fb950);
		transition: width 0.2s var(--ease);
	}
	.bar-fill.warm {
		background: var(--warn, #d29922);
	}
	.bar-fill.hot {
		background: var(--danger, #f85149);
	}
</style>
