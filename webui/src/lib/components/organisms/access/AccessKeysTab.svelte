<script lang="ts">
	import type { ApiKeyRow } from '@bindings/ApiKeyRow';
	import {
		Button,
		type Column,
		ConfirmModal,
		DataTable,
		Icon,
		IconButton,
		Text,
		Timestamp
	} from '@dorsk/tsumikit';
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

	const columns: Column<ApiKeyRow>[] = [
		{ key: 'label', label: m.access_col_key(), role: 'title' },
		{ key: 'preview', label: m.access_col_preview(), width: '9rem', role: 'detail', hideBelow: 'md' },
		{ key: 'scopes', label: m.access_col_scopes(), role: 'meta' },
		{ key: 'created', label: m.users_col_created(), width: '7rem', role: 'meta', hideBelow: 'sm' },
		{ key: 'used', label: m.users_col_last_used(), width: '7rem', role: 'meta' }
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

{#snippet colLabel(k: ApiKeyRow)}
	<span class="lead">
		<span class="glyph" title={k.kind}><Icon name={keyIcon(k.kind)} size={15} /></span>
		{#if k.kind === 'machine'}
			<MachineBadge name={k.label ?? k.id} id={k.id} hue={null} />
		{:else}
			<span class="nm">{k.label ?? m.users_unlabeled()}</span>
		{/if}
	</span>
{/snippet}
{#snippet colPreview(k: ApiKeyRow)}
	<span class="mono faint">{k.key_preview ?? '••••'}</span>
{/snippet}
{#snippet colScopes(k: ApiKeyRow)}
	<span class="scopes">
		{#each scopeCells(k.scopes) as s (s.name)}
			<span class:missing={!s.granted}>{s.name}</span>
		{/each}
	</span>
{/snippet}
{#snippet colCreated(k: ApiKeyRow)}
	<Timestamp value={k.created_at} mode="short-iso" mono size="xs" tone="faint" />
{/snippet}
{#snippet colUsed(k: ApiKeyRow)}
	{#if k.last_used_at}
		<Timestamp value={k.last_used_at} mode="relative" size="xs" tone="faint" />
	{:else}
		<Text size="xs" tone="faint">{m.users_never_used()}</Text>
	{/if}
{/snippet}
{#snippet colActions(k: ApiKeyRow)}
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
	{/if}
{/snippet}

<section class="tbl">
	<div class="bar">
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
	</div>
	<DataTable
		{columns}
		{rows}
		rowKey={(k) => k.id}
		responsive="stack"
		style="border: 0; border-radius: 0"
		loading={keys.isLoading}
		loadingLabel={m.common_loading()}
		empty={m.access_keys_empty()}
		rowClass={(k) => (k.revoked_at ? 'row-dim' : undefined)}
		rowActions={colActions}
		rowActionsLabel={m.common_actions()}
		cellSnippets={{
			label: colLabel,
			preview: colPreview,
			scopes: colScopes,
			created: colCreated,
			used: colUsed
		}}
	/>
</section>

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
	.tbl {
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		overflow: hidden;
	}
	.bar {
		display: flex;
		align-items: center;
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-4);
		border-bottom: 1px solid var(--border);
	}
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
		gap: var(--sp-2);
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
</style>
