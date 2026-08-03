<script lang="ts">
	import type { SoftLimitConfig } from '$lib/queries';
	import { useAccountUsage } from '$lib/queries';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import SoftLimit from '$lib/components/molecules/SoftLimit.svelte';
	import { mergeUsageWindows } from '$lib/components/molecules/usage-windows';

	// Per-account subscription usage shown as horizontal bars:
	// one SoftLimit row per normalized usage window, plus a separate section for
	// caps configured on windows the latest response didn't report. Reuses the
	// lazy/slow-refresh fetch; renders nothing for providers without a usage API.
	let {
		id,
		provider,
		enabled = true,
		softLimits = null
	}: {
		id: string;
		provider: string;
		enabled?: boolean;
		/** Configured caps, merged onto the matching window by key. */
		softLimits?: Record<string, SoftLimitConfig> | null;
	} = $props();

	const active = $derived(
		enabled && (provider === 'anthropic' || provider === 'openai' || provider === 'fireworks')
	);
	const q = useAccountUsage(
		() => id,
		() => active
	);

	const rows = $derived(mergeUsageWindows(q.data?.windows ?? [], softLimits));
	const hasRows = $derived(rows.observed.length > 0 || rows.unobserved.length > 0);
</script>

{#if !active}
	<!-- provider without a usage API: nothing to show -->
{:else if q.isLoading}
	<span class="spin"></span>
{:else if q.isError}
	<Text tone="danger" size="xs">{m.sessions_usage_error()}</Text>
{:else if hasRows}
	<div class="bars">
		{#each rows.observed as r (r.key)}
			<SoftLimit
				label={r.label}
				utilization={r.utilization}
				amountUsd={r.amountUsd}
				resets={r.resets}
				cap={r.cap}
				capUsd={r.capUsd}
				bypass={r.bypass}
				usd={r.usd}
			/>
		{/each}
		{#if rows.unobserved.length}
			<Text size="xs" tone="faint">{m.sessions_usage_configured_unreported()}</Text>
			{#each rows.unobserved as r (r.key)}
				<SoftLimit
				label={r.label}
				utilization={null}
				cap={r.cap}
				capUsd={r.capUsd}
				bypass={r.bypass}
				observed={false}
				usd={r.usd}
			/>
			{/each}
		{/if}
	</div>
{:else}
	<Text tone="faint">{m.sessions_no_usage_data()}</Text>
{/if}

<style>
	.bars {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
