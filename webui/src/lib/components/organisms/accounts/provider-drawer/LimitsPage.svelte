<script lang="ts">
	import { Input, Text } from '@dorsk/tsumikit';
	import SoftLimit from '$lib/components/molecules/SoftLimit.svelte';
	import { m } from '$lib/paraglide/messages';
	import type { UsageWindow } from '$lib/queries';
	import type { SoftEdit } from '../account-editor.logic';
	import { isUsdKey } from '$lib/components/molecules/usage-windows';

	let {
		rows,
		windows = [],
		edits = $bindable({}),
		rate = $bindable({ rpm: null, tpm: null })
	}: {
		rows: { key: string; label: string }[];
		windows?: UsageWindow[];
		edits?: Record<string, SoftEdit>;
		rate?: { rpm: number | null; tpm: number | null };
	} = $props();

	const live = $derived(new Map(windows.map((w) => [w.key, w])));
</script>

<div class="page">
	<Text as="p" tone="muted" size="sm">{m.limits_intro()}</Text>

	{#each rows as row (row.key)}
		{#if edits[row.key]}
			{@const w = live.get(row.key)}
			<SoftLimit
				label={row.label}
				utilization={w?.utilization ?? null}
				amountUsd={w?.amount_usd ?? null}
				resets={w?.resets_at ?? null}
				usd={isUsdKey(row.key)}
				pace={w?.pace ?? null}
				editable
				bind:cap={edits[row.key].cap}
				bind:capUsd={edits[row.key].capUsd}
				bind:bypass={edits[row.key].bypass}
			/>
		{/if}
	{/each}
	<div class="rates">
		<label class="rate">
			<Text as="span" size="xs" tone="muted">{m.accounts_rate_rpm_label()}</Text>
			<Input
				type="number"
				min="0"
				step="1"
				size="sm"
				mono
				bind:value={rate.rpm}
				placeholder={m.limits_unlimited()}
			/>
		</label>
		<label class="rate">
			<Text as="span" size="xs" tone="muted">{m.accounts_rate_tpm_label()}</Text>
			<Input
				type="number"
				min="0"
				step="1"
				size="sm"
				mono
				bind:value={rate.tpm}
				placeholder={m.limits_unlimited()}
			/>
		</label>
	</div>
	<Text as="p" tone="faint" size="xs">{m.accounts_rate_limits_help()}</Text>

</div>

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.rates {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--sp-2);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
	.rate {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		min-width: 0;
	}
</style>
