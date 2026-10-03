<script lang="ts">
	import { useCacheLoss } from '$lib/queries';
	import { compact, usd } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import { Card, Cluster, Stack, Text } from '@dorsk/tsumikit';
	import {
		CACHE_LOSS_DAYS,
		CACHE_LOSS_REASONS,
		cacheLossRows,
		cacheLossTotals,
		type CacheLossReason
	} from './cache-loss';

	let { days = CACHE_LOSS_DAYS }: { days?: number } = $props();

	const windowDays = $derived(Math.min(days, CACHE_LOSS_DAYS));
	const q = useCacheLoss(() => windowDays);
	const rows = $derived(cacheLossRows(q.data ?? []));
	const totals = $derived(cacheLossTotals(q.data ?? []));
	const reasonLabel = (r: CacheLossReason) =>
		r === 'ttl_expired'
			? m.home_cache_loss_ttl_expired()
			: r === 'gateway_rewrote_body'
				? m.home_cache_loss_gateway_rewrote_body()
				: m.home_cache_loss_unknown();
</script>

<Card>
	<Stack gap="var(--sp-3)">
		<Cluster gap="var(--sp-3)" align="baseline">
			<Text size="sm" weight="semibold">
				{windowDays === 1 ? m.home_cache_loss_title_24h() : m.home_cache_loss_title_7d()}
			</Text>
			<Text size="xs" tone="faint" numeric>{compact(totals.tokens.total)}</Text>
			{#if totals.busts > 0}
				<Text size="xs" tone="faint" numeric>{m.home_cache_loss_busts({ count: totals.busts })}</Text>
			{/if}
			{#if totals.usd > 0}
				<Text size="xs" tone="faint" numeric>{usd(totals.usd)}</Text>
			{/if}
		</Cluster>
		{#if q.isLoading}
			<Text tone="faint" size="sm">{m.common_loading()}</Text>
		{:else if rows.length === 0}
			<Text tone="faint" size="sm">{m.home_cache_loss_none()}</Text>
		{:else}
			<Cluster gap="var(--sp-3)">
				{#each CACHE_LOSS_REASONS as r (r)}
					<span class="legend">
						<span class="swatch {r}"></span>
						<Text size="xs" tone="muted">{reasonLabel(r)} · {compact(totals.tokens[r])}</Text>
					</span>
				{/each}
			</Cluster>
			<div class="rows">
				{#each rows as row (row.day)}
					<Text size="xs" tone="muted" numeric>{row.day}</Text>
					<div class="bar" role="img" aria-label={`${row.day}: ${compact(row.tokens)}`}>
						{#each CACHE_LOSS_REASONS as r (r)}
							<span class="seg {r}" style:width={`${row.widths[r]}%`}></span>
						{/each}
					</div>
					<Text size="xs" numeric>
						{compact(row.tokens)}{row.usd > 0 ? ` · ${usd(row.usd)}` : ''}
					</Text>
				{/each}
			</div>
		{/if}
	</Stack>
</Card>

<style>
	.rows {
		display: grid;
		grid-template-columns: auto minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--sp-1) var(--sp-2);
	}
	.bar {
		display: flex;
		height: 0.625rem;
		border-radius: var(--r-sm);
		background: var(--border);
		overflow: hidden;
	}
	.legend {
		display: inline-flex;
		align-items: center;
		gap: var(--sp-1);
	}
	.swatch {
		width: 0.625rem;
		height: 0.625rem;
		border-radius: var(--r-sm);
	}
	.ttl_expired {
		background: var(--warn);
	}
	.gateway_rewrote_body {
		background: var(--danger);
	}
	.unknown {
		background: var(--text-faint);
	}
</style>
