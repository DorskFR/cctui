<script lang="ts">
	import type { UserTokenRow } from '@bindings/UserTokenRow';
	import { Button, type Column, ConfirmModal, DataTable, IconButton, Text, Timestamp } from '@dorsk/tsumikit';
	import EditEntityModal from '$lib/components/molecules/EditEntityModal.svelte';
	import RowActions from '$lib/components/molecules/RowActions.svelte';
	import { useTokens, useUserActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import { splitRevoked, visibleRows } from './access.logic';

	let {
		userId,
		userName,
		canManage,
		onsecret
	}: {
		userId: string;
		userName: string;
		canManage: boolean;
		onsecret: (title: string, value: string) => void;
	} = $props();

	const tokens = useTokens(
		() => userId,
		() => true
	);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: unknown) => toasts.error(errMessage(e)));

	let showRevoked = $state(false);
	let mintOpen = $state(false);
	let relabel = $state<UserTokenRow | null>(null);
	let dropTarget = $state<UserTokenRow | null>(null);

	const all = $derived(tokens.data ?? []);
	const groups = $derived(splitRevoked(all));
	const rows = $derived(visibleRows(all, showRevoked));

	const columns: Column<UserTokenRow>[] = [
		{ key: 'label', label: m.users_col_token(), role: 'title' },
		{ key: 'status', label: m.users_col_status(), width: '7rem', role: 'meta' },
		{ key: 'created', label: m.users_col_created(), width: '16rem', role: 'meta' }
	];

	function mint(label: string | null) {
		guard(
			actions
				.mintToken(userId, label)
				.then((r) => onsecret(m.users_token_secret_title({ name: userName }), r.token))
		);
	}
	function drop(t: UserTokenRow) {
		guard(t.revoked_at ? actions.purgeToken(userId, t.id) : actions.revokeToken(userId, t.id));
		dropTarget = null;
	}
</script>

{#snippet colLabel(t: UserTokenRow)}
	<span class="id">
		<span class="nm">{t.label || m.users_unlabeled()}</span>
		<span class="mono faint">{t.token_preview ?? '••••••••'}</span>
	</span>
{/snippet}
{#snippet colStatus(t: UserTokenRow)}
	<Text size="xs" tone={t.revoked_at ? 'danger' : 'muted'}>
		{t.revoked_at ? m.users_badge_revoked() : m.users_badge_active()}
	</Text>
{/snippet}
{#snippet colCreated(t: UserTokenRow)}
	<span class="stamp">
		<Timestamp value={t.created_at} mode="short-iso" mono size="xs" tone="faint" details={false} />
		{#if t.expires_at}
			<Text size="xs" tone="faint">{m.users_expires_prefix()}</Text>
			<Timestamp value={t.expires_at} mode="short-iso" mono size="xs" tone="faint" details={false} />
		{/if}
	</span>
{/snippet}
{#snippet colActions(t: UserTokenRow)}
	{#if canManage}
		<RowActions>
			{#if !t.revoked_at}
				<IconButton
					inline
					icon="edit"
					size={14}
					label={m.users_relabel()}
					title={m.users_relabel()}
					onclick={() => (relabel = t)}
				/>
			{/if}
			<IconButton
				inline
				hoverDanger
				icon="trash"
				size={14}
				label={t.revoked_at ? m.common_delete() : m.users_revoke()}
				title={t.revoked_at ? m.common_delete() : m.users_revoke()}
				onclick={() => (dropTarget = t)}
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
			<Button variant="primary" size="sm" onclick={() => (mintOpen = true)}>{m.users_new_token()}</Button>
		{/if}
	</div>
	<DataTable
		{columns}
		{rows}
		rowKey={(t) => t.id}
		responsive="stack"
		style="border: 0; border-radius: 0"
		loading={tokens.isLoading}
		loadingLabel={m.common_loading()}
		empty={m.users_tokens_empty()}
		rowClass={(t) => (t.revoked_at ? 'row-dim' : undefined)}
		rowActions={colActions}
		rowActionsLabel={m.common_actions()}
		cellSnippets={{ label: colLabel, status: colStatus, created: colCreated }}
	/>
</section>

{#if mintOpen}
	<EditEntityModal
		title={m.users_new_token_title()}
		fieldLabel={m.users_field_label_optional()}
		placeholder={m.users_token_placeholder()}
		saveLabel={m.users_create_token()}
		onsave={(label) => mint(label)}
		onclose={() => (mintOpen = false)}
	/>
{/if}

{#if relabel}
	{@const t = relabel}
	<EditEntityModal
		title={m.users_relabel_token()}
		fieldLabel={m.users_field_label()}
		name={t.label}
		placeholder={m.users_unlabeled()}
		onsave={(label) => guard(actions.relabelToken(userId, t.id, label))}
		onclose={() => (relabel = null)}
	/>
{/if}

{#if dropTarget}
	{@const t = dropTarget}
	<ConfirmModal
		open
		tone="danger"
		title={t.revoked_at ? m.common_delete() : m.users_revoke()}
		message={t.revoked_at ? m.users_confirm_delete_token() : m.users_confirm_revoke_token()}
		confirmLabel={t.revoked_at ? m.common_delete() : m.users_revoke()}
		onconfirm={() => drop(t)}
		oncancel={() => (dropTarget = null)}
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
	.id {
		min-width: 0;
		display: flex;
		flex-direction: column;
	}
	.nm {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.stamp {
		min-width: 0;
		display: inline-flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-1);
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
</style>
