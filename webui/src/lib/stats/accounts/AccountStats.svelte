<script lang="ts">
	import { Field, Heading, Select, Text } from '@dorsk/tsumikit';
	import { now as clockNow } from '$lib/clock.svelte';
	import { getLocale } from '$lib/paraglide/runtime';
	import { m } from '$lib/paraglide/messages';
	import { providerLabel } from '$lib/providers';
	import AdapterIcon from '$lib/components/atoms/AdapterIcon.svelte';
	import { countdown } from '$lib/components/molecules/usage-battery.logic';
	import StatMeter from '../StatMeter.svelte';
	import LedgerTable from './LedgerTable.svelte';
	import { useAccountLedger, useAllAccountsUsage } from './data.svelte';
	import {
		currentRows,
		filterLedger,
		ledgerOptions,
		oneLiners,
		resetClock,
		totalGroups,
		type CurrentWindow
	} from './model';

	let {
		size = 'full',
		account = null
	}: {
		size?: 'full' | 'light' | 'mini';
		/** Narrow to one credential (a provider row id). */
		account?: string | null;
	} = $props();

	const ledger = useAccountLedger();
	const usage = useAllAccountsUsage(() => size !== 'mini');

	const now = $derived(clockNow(60_000));
	const clock = (iso: string) => resetClock(iso, now, getLocale());

	const ledgerRows = $derived(
		filterLedger(ledger.data?.rows ?? [], { account, window: null })
	);
	const current = $derived(
		currentRows(usage.data, providerLabel).filter((r) => !account || r.id === account)
	);
	const currentById = $derived(new Map(current.map((r) => [r.id, r])));
	const lines = $derived(oneLiners(ledgerRows, clock, providerLabel));

	let pickedAccount = $state('');
	let pickedWindow = $state('');
	const options = $derived(ledgerOptions(ledgerRows, providerLabel));
	const tableRows = $derived(
		filterLedger(ledgerRows, { account: pickedAccount || null, window: pickedWindow || null })
	);
	const totals = $derived(
		totalGroups(
			(ledger.data?.totals ?? []).filter((t) => !account || t.account_id === account),
			ledgerRows,
			providerLabel
		)
	);

	const resetCaption = (w: CurrentWindow | null) => {
		const ms = w?.resetsAt ? Date.parse(w.resetsAt) - now : null;
		return ms !== null && Number.isFinite(ms)
			? m.usage_battery_resets_in({ time: countdown(ms) })
			: undefined;
	};
</script>

{#snippet meters(id: string, meterSize: 'light' | 'mini')}
	{@const c = currentById.get(id)}
	{#if c}
		<StatMeter
			kind="quota"
			size={meterSize}
			label={m.usage_battery_five_hour()}
			value={c.fiveHour?.pct ?? null}
			caption={meterSize === 'light' ? resetCaption(c.fiveHour) : undefined}
		/>
		<StatMeter
			kind="quota"
			size={meterSize}
			label={m.usage_battery_weekly()}
			value={c.weekly?.pct ?? null}
			caption={meterSize === 'light' ? resetCaption(c.weekly) : undefined}
		/>
	{/if}
{/snippet}

{#if size === 'mini'}
	{#if lines.length > 0}
		<div class="mini" data-stats="accounts-mini">
			{#each lines as l (l.id)}
				<Text size="xs" tone={l.tone === 'unknown' ? 'faint' : 'muted'}>{l.text}</Text>
			{/each}
		</div>
	{/if}
{:else if size === 'light'}
	<div class="light" data-stats="accounts-light">
		{#each lines as l (l.id)}
			<div class="line">
				<span class="bars">{@render meters(l.id, 'mini')}</span>
				<Text size="sm" tone={l.tone === 'unknown' ? 'faint' : 'default'}>{l.text}</Text>
			</div>
		{:else}
			<Text size="sm" tone="faint">
				{ledger.isLoading ? m.common_loading() : m.account_stats_empty()}
			</Text>
		{/each}
	</div>
{:else}
	<section class="full" data-stats="accounts-full" aria-label={m.account_stats_title()}>
		<Heading level={3} size="sm">{m.account_stats_current()}</Heading>
		{#if current.length === 0}
			<Text size="sm" tone="faint">
				{usage.isLoading ? m.common_loading() : m.account_stats_no_usage()}
			</Text>
		{:else}
			<div class="current">
				{#each current as c (c.id)}
					<div class="cred">
						<span class="who">
							<AdapterIcon provider={c.provider} size={14} />
							<Text size="sm" weight="semibold" truncate>
								{c.emoji ? `${c.emoji} ` : ''}{c.label}
							</Text>
						</span>
						{@render meters(c.id, 'light')}
					</div>
				{/each}
			</div>
		{/if}

		<Heading level={3} size="sm">{m.account_stats_ledger()}</Heading>
		{#if totals.length > 0}
			<div class="totals">
				{#each totals as g (g.id)}
					<div class="total">
						<Text size="sm" weight="semibold">{g.label}</Text>
						{#each g.lines as line, i (i)}
							<Text size="sm" tone="muted">{line}</Text>
						{/each}
					</div>
				{/each}
			</div>
		{/if}
		<div class="filters">
			<Field label={m.account_stats_col_account()}>
				<Select bind:value={pickedAccount}>
					<option value="">{m.account_stats_all_accounts()}</option>
					{#each options.accounts as o (o.value)}
						<option value={o.value}>{o.label}</option>
					{/each}
				</Select>
			</Field>
			<Field label={m.account_stats_col_window()}>
				<Select bind:value={pickedWindow}>
					<option value="">{m.account_stats_all_windows()}</option>
					{#each options.windows as o (o.value)}
						<option value={o.value}>{o.label}</option>
					{/each}
				</Select>
			</Field>
		</div>
		{#if ledger.isLoading}
			<Text size="sm" tone="faint">{m.common_loading()}</Text>
		{:else if ledger.isError}
			<Text size="sm" tone="danger">{m.account_stats_error()}</Text>
		{:else}
			<LedgerTable rows={tableRows} {clock} />
		{/if}
	</section>
{/if}

<style>
	.mini,
	.light,
	.totals {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.light {
		gap: var(--sp-2);
	}
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.bars {
		display: inline-flex;
		flex-direction: column;
		gap: var(--sp-1);
		flex: none;
	}
	.full {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		min-width: 0;
	}
	.current {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr));
		gap: var(--sp-3);
	}
	.cred {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		min-width: 0;
	}
	.who {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		min-width: 0;
	}
	.total {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 0 var(--sp-3);
	}
	.filters {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-3);
	}
</style>
