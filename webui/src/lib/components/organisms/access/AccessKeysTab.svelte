<script lang="ts">
	import type { ApiKeyRow } from '@bindings/ApiKeyRow';
	import { Button, ConfirmModal, Icon, IconButton, Text, Timestamp } from '@dorsk/tsumikit';
	import AccessTable, { type AccessColumn } from '$lib/components/molecules/AccessTable.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import RowActions from '$lib/components/molecules/RowActions.svelte';
	import { useUserActions, useUserKeys } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import KeyScopesModal from './KeyScopesModal.svelte';
	import { keyIcon, scopeCells, splitRevoked, visibleRows } from './access.logic';

	let {
		userId,
		ceiling,
		canManage,
		onsecret
	}: {
		userId: string;
		ceiling: ReadonlySet<string>;
		canManage: boolean;
		onsecret: (title: string, value: string) => void;
	} = $props();

	const keys = useUserKeys(() => userId);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: unknown) => toasts.error(errMessage(e)));

	let showRevoked = $state(false);
	let mintOpen = $state(false);
	let editKey = $state<ApiKeyRow | null>(null);
	let revokeTarget = $state<ApiKeyRow | null>(null);

	const all = $derived(keys.data ?? []);
	const groups = $derived(splitRevoked(all));
	const rows = $derived(visibleRows(all, showRevoked));

	const columns: AccessColumn[] = [
		{ key: 'icon', width: '24px' },
		{ key: 'label', label: m.access_col_key(), width: 'minmax(0, 1.3fr)' },
		{ key: 'preview', label: m.access_col_preview(), width: 'minmax(0, 1fr)' },
		{ key: 'scopes', label: m.access_col_scopes(), width: 'minmax(0, 1.6fr)' },
		{ key: 'created', label: m.users_col_created(), width: '96px' },
		{ key: 'used', label: m.users_col_last_used(), width: '88px' },
		{ key: 'actions', width: '56px' }
	];

	function mint(label: string | null, scopes: string[]) {
		guard(
			actions
				.mintKey(userId, label, scopes)
				.then((r) =>
					onsecret(m.users_key_secret_title({ label: label ?? m.users_unlabeled_plain() }), r.key)
				)
		);
	}
	function saveScopes(key: ApiKeyRow, scopes: string[]) {
		guard(actions.setKeyScopes(userId, key.id, scopes).then(() => toasts.ok(m.users_key_scopes_updated())));
	}
	function revoke(key: ApiKeyRow) {
		guard(actions.revokeKey(userId, key.id).then(() => toasts.ok(m.users_key_revoked())));
		revokeTarget = null;
	}
</script>

<AccessTable
	{columns}
	{rows}
	rowKey={(k) => k.id}
	loading={keys.isLoading}
	empty={m.access_keys_empty()}
	dim={(k) => !!k.revoked_at}
>
	{#snippet bar()}
		<Text size="xs" tone="faint"
			>{m.access_counts({ active: groups.active.length, revoked: groups.revoked.length })}</Text
		>
		<div class="spacer"></div>
		{#if groups.revoked.length}
			<Button variant="link" size="sm" onclick={() => (showRevoked = !showRevoked)}>
				{showRevoked ? m.access_hide_revoked() : m.access_show_revoked()}
			</Button>
		{/if}
		{#if canManage}
			<Button variant="primary" size="sm" onclick={() => (mintOpen = true)}>{m.users_mint_key()}</Button>
		{/if}
	{/snippet}

	{#snippet row(k: ApiKeyRow)}
		<span class="glyph" title={k.kind}><Icon name={keyIcon(k.kind)} size={15} /></span>
		<span class="lead">
			{#if k.kind === 'machine'}
				<MachineBadge name={k.label ?? k.id} id={k.id} hue={null} />
			{:else}
				<span class="nm">{k.label ?? m.users_unlabeled()}</span>
			{/if}
		</span>
		<span class="mono faint">{k.key_preview ?? '••••'}</span>
		<span class="scopes">
			{#each scopeCells(k.scopes) as s (s.name)}
				<span class:missing={!s.granted}>{s.name}</span>
			{/each}
		</span>
		<span class="stamp">
			<Timestamp value={k.created_at} mode="short-iso" mono size="xs" tone="faint" details={false} />
		</span>
		<span class="stamp">
			{#if k.last_used_at}
				<Timestamp value={k.last_used_at} mode="relative" size="xs" tone="faint" details={false} />
			{:else}
				<Text size="xs" tone="faint">{m.users_never_used()}</Text>
			{/if}
		</span>
		{#if canManage && !k.revoked_at}
			<RowActions>
				<IconButton
					inline
					icon="edit"
					size={14}
					label={m.access_edit_scopes()}
					title={m.access_edit_scopes()}
					onclick={() => (editKey = k)}
				/>
				<IconButton
					inline
					hoverDanger
					icon="trash"
					size={14}
					label={m.users_revoke_key()}
					title={m.users_revoke_key()}
					onclick={() => (revokeTarget = k)}
				/>
			</RowActions>
		{:else}
			<span></span>
		{/if}
	{/snippet}
</AccessTable>

{#if mintOpen}
	<KeyScopesModal
		title={m.access_mint_key_title()}
		withLabel
		{ceiling}
		saveLabel={m.users_mint_key()}
		onsave={mint}
		onclose={() => (mintOpen = false)}
	/>
{/if}

{#if editKey}
	{@const k = editKey}
	<KeyScopesModal
		title={m.access_edit_scopes()}
		scopes={k.scopes}
		{ceiling}
		onsave={(_l, scopes) => saveScopes(k, scopes)}
		onclose={() => (editKey = null)}
	/>
{/if}

{#if revokeTarget}
	{@const k = revokeTarget}
	<ConfirmModal
		open
		tone="danger"
		title={m.users_revoke_key()}
		message={m.users_confirm_revoke_key({ key: k.label ?? k.key_preview ?? k.id })}
		confirmLabel={m.users_revoke()}
		onconfirm={() => revoke(k)}
		oncancel={() => (revokeTarget = null)}
	/>
{/if}

<style>
	.spacer {
		flex: 1;
	}
	.glyph {
		display: grid;
		place-items: center;
		color: var(--text-faint);
	}
	.lead {
		min-width: 0;
		display: flex;
		align-items: center;
	}
	.nm {
		font-weight: var(--fw-medium);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.mono {
		font-family: var(--font-mono);
		font-size: var(--fs-xs);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.faint {
		color: var(--text-faint);
	}
	.scopes {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
		font-size: var(--fs-xs);
		color: var(--text);
	}
	.scopes .missing {
		color: var(--text-faint);
		text-decoration: line-through;
	}
	.stamp {
		min-width: 0;
	}
</style>
