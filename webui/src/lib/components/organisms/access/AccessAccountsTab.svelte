<script lang="ts">
	import { Button, Text, Timestamp } from '@dorsk/tsumikit';
	import AccessTable, { type AccessColumn } from '$lib/components/molecules/AccessTable.svelte';
	import AccountAvatar from '$lib/components/molecules/AccountAvatar.svelte';
	import RowActions from '$lib/components/molecules/RowActions.svelte';
	import type { OAuthAccount } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	let {
		accounts,
		loading = false
	}: {
		accounts: OAuthAccount[];
		loading?: boolean;
	} = $props();

	const columns: AccessColumn[] = [
		{ key: 'account', label: m.access_col_account(), width: 'minmax(0, 1.4fr)' },
		{ key: 'providers', label: m.access_col_providers(), width: 'minmax(0, 1.4fr)' },
		{ key: 'created', label: m.users_col_created(), width: '96px' },
		{ key: 'actions', width: '56px' }
	];
</script>

<AccessTable
	{columns}
	rows={accounts}
	rowKey={(a) => a.id}
	{loading}
	empty={m.access_accounts_empty()}
>
	{#snippet row(a: OAuthAccount)}
		<span class="lead">
			<AccountAvatar emoji={a.emoji} name={a.name} id={a.id} size={20} decorative />
			<span class="nm">{a.name}</span>
		</span>
		<span class="providers">
			<Text size="xs" tone="faint"
				>{a.providers.map((p) => p.provider).join(' · ') || m.access_no_providers()}</Text
			>
		</span>
		<span class="stamp">
			<Timestamp value={a.created_at} mode="short-iso" mono size="xs" tone="faint" details={false} />
		</span>
		<RowActions>
			<Button variant="link" size="sm" as="a" href="/accounts">{m.access_open()}</Button>
		</RowActions>
	{/snippet}
</AccessTable>

<style>
	.lead {
		min-width: 0;
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.nm {
		font-weight: var(--fw-medium);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.providers,
	.stamp {
		min-width: 0;
	}
</style>
