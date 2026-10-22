<script lang="ts">
	import { DataTable, Text, type Column } from '@dorsk/tsumikit';
	import type { AccountLedgerRow } from '@bindings/AccountLedgerRow';
	import { providerLabel } from '$lib/providers';
	import { m } from '$lib/paraglide/messages';
	import {
		credentialLabels,
		hoursText,
		ledgerRowTone,
		lossTone,
		lostText,
		sortLedger,
		usdText,
		windowLabel,
		type LedgerSortKey
	} from './model';

	let { rows, clock }: { rows: AccountLedgerRow[]; clock: (iso: string) => string } = $props();

	let sort = $state<{ key: LedgerSortKey; dir: 'asc' | 'desc' }>({ key: 'resets_at', dir: 'desc' });
	const sorted = $derived(sortLedger(rows, sort.key, sort.dir));
	const labels = $derived(
		credentialLabels(
			rows.map((r) => ({ id: r.account_id, name: r.account_name, provider: r.provider })),
			providerLabel
		)
	);
	const hasUsd = $derived(rows.some((r) => r.amount_usd !== null));
	const LOSS_TONE = { ok: 'default', warn: 'warn', danger: 'danger', unknown: 'faint' } as const;

	const columns = $derived<Column<AccountLedgerRow>[]>([
		{
			key: 'account',
			label: m.account_stats_col_account(),
			role: 'title',
			sortable: true,
			get: (r) => `${r.account_emoji ? `${r.account_emoji} ` : ''}${labels.get(r.account_id) ?? r.account_name}`
		},
		{
			key: 'window',
			label: m.account_stats_col_window(),
			sortable: true,
			get: (r) => windowLabel(r.window_key, r.window_label)
		},
		{
			key: 'resets_at',
			label: m.account_stats_col_reset(),
			sortable: true,
			nowrap: true,
			get: (r) => clock(r.resets_at)
		},
		{
			key: 'used',
			label: m.account_stats_col_used(),
			align: 'right',
			sortable: true,
			get: (r) => (r.idle ? '–' : `${Math.round(r.final_utilization)}%`)
		},
		{ key: 'lost', label: m.account_stats_col_lost(), align: 'right', sortable: true },
		{
			key: 'hours',
			label: m.account_stats_col_hours(),
			align: 'right',
			sortable: true,
			hideBelow: 'sm',
			get: (r) => (r.idle ? '–' : (hoursText(r.unused_capacity_hours) ?? '–'))
		},
		...(hasUsd
			? [
					{
						key: 'usd',
						label: m.account_stats_col_usd(),
						align: 'right' as const,
						hideBelow: 'md' as const,
						get: (r: AccountLedgerRow) => usdText(r.amount_usd) ?? '–'
					}
				]
			: [])
	]);
</script>

{#snippet lost(r: AccountLedgerRow)}
	<Text
		size="sm"
		numeric
		tone={LOSS_TONE[lossTone(r)]}
		title={r.sampling_gap ? m.account_stats_sampling_gap() : undefined}
	>
		{lostText(r)}
	</Text>
{/snippet}

<DataTable
	{columns}
	rows={sorted}
	rowKey={(r) => `${r.account_id}:${r.window_key}:${r.resets_at}`}
	rowTone={ledgerRowTone}
	cellSnippets={{ lost }}
	onsort={(key, dir) => (sort = { key: key as LedgerSortKey, dir })}
	empty={m.account_stats_empty()}
	size="sm"
	responsive="stack"
	stickyHeader
	style="max-height: 28rem"
/>
