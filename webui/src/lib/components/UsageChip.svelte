<script lang="ts">
	import { useAccountUsage } from '$lib/queries';
	import { relativeTime } from '$lib/format';

	// Per-account subscription-usage chip (CCT-306). Shows Anthropic's free OAuth
	// usage windows (5h session + 7d weekly utilization) for claude accounts;
	// hides entirely for providers with no usage API (Codex) or while we have no
	// data. Fetch is lazy + slow-refresh (see useAccountUsage) so the rate-limited
	// upstream is never spammed.
	let {
		id,
		provider,
		enabled = true
	}: {
		id: string;
		provider: string;
		/** Gate the fetch (e.g. only while the accounts view is mounted). */
		enabled?: boolean;
	} = $props();

	// Codex/OpenAI has no free usage endpoint — never fetch for it.
	const active = $derived(enabled && provider === 'anthropic');
	const q = useAccountUsage(
		() => id,
		() => active
	);

	const usage = $derived($q.data?.usage ?? null);
	const fiveHour = $derived(usage?.five_hour ?? null);
	const sevenDay = $derived(usage?.seven_day ?? null);

	const pct = (v: number | null | undefined) =>
		v === null || v === undefined ? null : Math.round(v);

	const fivePct = $derived(pct(fiveHour?.utilization));
	const sevenPct = $derived(pct(sevenDay?.utilization));

	// Color the 5h chip by how close to the cap we are.
	const tone = $derived(
		fivePct === null ? '' : fivePct >= 90 ? 'hot' : fivePct >= 70 ? 'warm' : 'ok'
	);

	const tip = $derived(
		[
			fiveHour
				? `5h: ${fivePct}%${fiveHour.resets_at ? ` · resets ${relativeTime(fiveHour.resets_at)}` : ''}`
				: null,
			sevenDay
				? `7d: ${sevenPct}%${sevenDay.resets_at ? ` · resets ${relativeTime(sevenDay.resets_at)}` : ''}`
				: null
		]
			.filter(Boolean)
			.join('\n')
	);
</script>

{#if active && (fivePct !== null || sevenPct !== null)}
	<span class="usage" class:hot={tone === 'hot'} class:warm={tone === 'warm'} title={tip}>
		{#if fivePct !== null}<span class="seg">5h {fivePct}%</span>{/if}
		{#if sevenPct !== null}<span class="seg seg-week">7d {sevenPct}%</span>{/if}
	</span>
{:else if active && $q.isLoading}
	<span class="spin"></span>
{:else}
	<span class="faint">—</span>
{/if}

<style>
	.usage {
		display: inline-flex;
		gap: 0.4rem;
		align-items: center;
		font-variant-numeric: tabular-nums;
		font-size: 0.85em;
		white-space: nowrap;
	}
	.seg {
		opacity: 0.85;
	}
	.seg-week {
		opacity: 0.6;
	}
	.usage.warm .seg {
		color: var(--warn, #d08700);
		opacity: 1;
	}
	.usage.hot .seg {
		color: var(--danger, #d33);
		opacity: 1;
		font-weight: 600;
	}
</style>
