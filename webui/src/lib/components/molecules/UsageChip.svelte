<script lang="ts">
	import { useAccountUsage } from '$lib/queries';
	import { relativeTime } from '$lib/format';
	import { Text } from '@dorsk/tsumikit';

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
	<span class="usage" title={tip}>
		{#if fivePct !== null}<Text
				numeric
				weight={tone === 'hot' ? 'semibold' : 'normal'}
				tone={tone === 'hot' ? 'danger' : tone === 'warm' ? 'warn' : 'muted'}>5h {fivePct}%</Text>{/if}
		{#if sevenPct !== null}<Text
				numeric
				weight={tone === 'hot' ? 'semibold' : 'normal'}
				tone={tone === 'hot' ? 'danger' : tone === 'warm' ? 'warn' : 'faint'}>7d {sevenPct}%</Text>{/if}
	</span>
{:else if active && $q.isLoading}
	<span class="spin"></span>
{:else}
	<Text tone="faint">—</Text>
{/if}

<style>
	/* Segment colour/dimming/weight now come from Text's `tone`/`weight` props
	   (severity → warn/danger, idle → muted/faint), so no :global reach-in. */
	.usage {
		display: inline-flex;
		gap: 0.4rem;
		align-items: center;
		font-size: 0.85em;
		white-space: nowrap;
	}
</style>
