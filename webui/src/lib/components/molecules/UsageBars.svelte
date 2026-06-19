<script lang="ts">
	import { useAccountUsage } from '$lib/queries';
	import { Progress, Text, Timestamp } from '@dorsk/tsumikit';

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
		enabled = true,
		cap5h = null,
		cap7d = null
	}: {
		id: string;
		provider: string;
		enabled?: boolean;
		/** Per-account soft-limit caps (CCT-411), drawn as a marker on the matching
		 *  bar so the configured ceiling is visible against live utilization. */
		cap5h?: number | null;
		cap7d?: number | null;
	} = $props();

	const active = $derived(enabled && provider === 'anthropic');
	const q = useAccountUsage(
		() => id,
		() => active
	);

	const usage = $derived($q.data?.usage ?? null);

	type Win = { utilization?: number | null; resets_at?: string | null } | null | undefined;
	function row(label: string, w: Win, cap: number | null = null) {
		const u = w?.utilization;
		if (u === null || u === undefined) return null;
		const pct = Math.max(0, Math.min(100, Math.round(u)));
		const tone = pct >= HOT_PCT ? 'hot' : pct >= WARN_PCT ? 'warm' : 'ok';
		// A configured soft-limit cap (CCT-411) becomes a marker on the bar; an
		// out-of-range value is ignored.
		const capPct = cap != null && cap >= 0 && cap <= 100 ? cap : null;
		return { label, pct, tone, resets: w?.resets_at ?? null, capPct };
	}

	// Map the severity name to the shared tsumikit tone vocabulary used by both
	// Text (percent label) and Progress (fill), so they stay in lockstep.
	const toneFor = (t: string) => (t === 'hot' ? 'danger' : t === 'warm' ? 'warn' : 'success');

	const bars = $derived(
		[
			row('5h', usage?.five_hour, cap5h),
			row('7d', usage?.seven_day, cap7d),
			row('7d Opus', usage?.seven_day_opus),
			row('7d Sonnet', usage?.seven_day_sonnet)
		].filter((r): r is NonNullable<typeof r> => r !== null)
	);
</script>

{#if active && bars.length}
	<div class="bars">
		{#each bars as b (b.label)}
			<div class="bar-row">
				<Text size="xs" tone="muted" numeric class="bar-label">{b.label}</Text>
				<Text size="xs" numeric tone={toneFor(b.tone)} class="bar-pct">
					{b.pct}%{#if b.resets}<Text tone="faint" class="bar-reset"> · resets <Timestamp value={b.resets} mode="relative" tone="faint" /></Text>{/if}
				</Text>
				<div class="track-wrap">
					<Progress value={b.pct} label={`${b.label} usage`} tone={toneFor(b.tone)} class="bar-track" />
					{#if b.capPct != null}
						<span
							class="cap-marker"
							style={`left: ${b.capPct}%`}
							title={`cctui soft limit: ${b.capPct}%`}
						></span>
					{/if}
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
	/* Both are rendered by tsumikit atoms; the typography/colour now comes from
	   Text's `numeric`/`tone` and Progress's `tone`, so only the residual grid
	   placement is reached in here — scoped under .bar-row so it can't leak. */
	.bar-row :global(.bar-pct) {
		justify-self: end;
	}
	.track-wrap {
		grid-column: 1 / -1;
		position: relative;
		margin-top: 0.25rem;
	}
	/* Soft-limit cap marker (CCT-411): a thin vertical line at the configured % so
	   the ceiling reads against the live fill. */
	.cap-marker {
		position: absolute;
		top: -1px;
		bottom: -1px;
		width: 2px;
		transform: translateX(-1px);
		background: var(--text-muted, currentColor);
		opacity: 0.8;
		pointer-events: none;
		border-radius: 1px;
	}
</style>
