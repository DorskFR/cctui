<script lang="ts">
	import { Button, type Column, DataTable, Text, Timestamp } from '@dorsk/tsumikit';
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

	const columns: Column<OAuthAccount>[] = [
		{ key: 'account', label: m.access_col_account(), role: 'title' },
		{ key: 'providers', label: m.access_col_providers(), role: 'detail' },
		{ key: 'created', label: m.users_col_created(), width: '7rem', role: 'meta' }
	];
</script>

{#snippet colAccount(a: OAuthAccount)}
	<span class="lead">
		<AccountAvatar emoji={a.emoji} name={a.name} id={a.id} size={20} decorative />
		<span class="nm">{a.name}</span>
	</span>
{/snippet}
{#snippet colProviders(a: OAuthAccount)}
	<Text size="xs" tone="faint"
		>{a.providers.map((p) => p.provider).join(' · ') || m.access_no_providers()}</Text
	>
{/snippet}
{#snippet colCreated(a: OAuthAccount)}
	<Timestamp value={a.created_at} mode="short-iso" mono size="xs" tone="faint" />
{/snippet}
{#snippet colActions(_a: OAuthAccount)}
	<RowActions>
		<Button variant="link" size="sm" as="a" href="/accounts">{m.access_open()}</Button>
	</RowActions>
{/snippet}

<DataTable
	{columns}
	rows={accounts}
	rowKey={(a) => a.id}
	responsive="stack"
	{loading}
	loadingLabel={m.common_loading()}
	empty={m.access_accounts_empty()}
	rowActions={colActions}
	rowActionsLabel={m.common_actions()}
	cellSnippets={{ account: colAccount, providers: colProviders, created: colCreated }}
/>

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
</style>
