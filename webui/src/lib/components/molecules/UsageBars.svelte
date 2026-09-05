<script lang="ts">
	import type { SoftLimitConfig } from '$lib/queries';
	import { useAccountActions, useAccountUsage } from '$lib/queries';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import SoftLimit from '$lib/components/molecules/SoftLimit.svelte';
	import { mergeUsageWindows } from '$lib/components/molecules/usage-windows';
	import { withCap } from '$lib/components/molecules/cap-bar.logic';

	// Per-provider subscription usage as cap bars: one SoftLimit row per
	// normalized usage window, plus a section for caps configured on windows the
	// latest response didn't report. Renders nothing for providers without a
	// usage API.
	let {
		id,
		provider,
		accountId = null,
		enabled = true,
		softLimits = null
	}: {
		id: string;
		provider: string;
		/** With the owning account, a dragged cap PATCHes the provider's soft limits. */
		accountId?: string | null;
		enabled?: boolean;
		/** Configured caps, merged onto the matching window by key. */
		softLimits?: Record<string, SoftLimitConfig> | null;
	} = $props();

	const actions = useAccountActions();
	const setCap = $derived(
		accountId === null
			? undefined
			: (key: string) => async (cap: number | null) => {
					try {
						await actions.updateProvider(accountId, id, {
							soft_limits: withCap(softLimits, key, cap)
						});
					} catch (e) {
						toasts.error(errMessage(e));
					}
				}
	);

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
				pace={r.pace}
				oncapchange={r.usd ? undefined : setCap?.(r.key)}
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
				usd={r.usd}
				oncapchange={r.usd ? undefined : setCap?.(r.key)}
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
