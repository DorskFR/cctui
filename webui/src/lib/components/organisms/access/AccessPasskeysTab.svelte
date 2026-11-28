<script lang="ts">
	import type { PasskeyRow } from '@bindings/PasskeyRow';
	import { Button, type Column, ConfirmModal, DataTable, Text, Timestamp } from '@dorsk/tsumikit';
	import { useUserActions, useUserPasskeys } from '#lib/queries/index.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { errMessage } from '#lib/api.ts';
	import { m } from '#lib/paraglide/messages.js';

	let {
		userId,
		userName,
		canManage
	}: {
		userId: string;
		userName: string;
		canManage: boolean;
	} = $props();

	const passkeys = useUserPasskeys(
		() => userId,
		() => true
	);
	const actions = useUserActions();

	let dropTarget = $state<PasskeyRow | null>(null);

	const rows = $derived(passkeys.data?.passkeys ?? []);

	const columns: Column<PasskeyRow>[] = [
		{ key: 'label', label: m.users_col_passkey(), role: 'title' },
		{ key: 'created', label: m.users_col_created(), width: '9rem', nowrap: true, role: 'meta' },
		{ key: 'used', label: m.users_col_last_used(), width: '9rem', nowrap: true, role: 'meta' },
		{ key: 'actions', label: '', width: '6rem', nowrap: true, align: 'right', role: 'actions' }
	];

	function drop(p: PasskeyRow) {
		actions
			.revokePasskey(userId, p.id)
			.then(() => toasts.ok(m.settings_passkeys_revoked()))
			.catch((e: unknown) => toasts.error(errMessage(e)));
		dropTarget = null;
	}
</script>

{#snippet colLabel(p: PasskeyRow)}
	<span class="id">
		<span class="nm">{p.label}</span>
		{#if !p.discoverable}
			<Text size="xs" tone="faint">{m.settings_passkeys_not_discoverable()}</Text>
		{/if}
	</span>
{/snippet}
{#snippet colCreated(p: PasskeyRow)}
	<Timestamp value={p.created_at} mode="short-iso" size="xs" tone="faint" />
{/snippet}
{#snippet colUsed(p: PasskeyRow)}
	{#if p.last_used_at}
		<Timestamp value={p.last_used_at} mode="relative" size="xs" tone="faint" />
	{:else}
		<Text size="xs" tone="faint">{m.settings_passkeys_never_used()}</Text>
	{/if}
{/snippet}
{#snippet colActions(p: PasskeyRow)}
	{#if canManage}
		<Button size="sm" variant="ghost" onclick={() => (dropTarget = p)}>{m.settings_passkeys_revoke()}</Button>
	{/if}
{/snippet}

<section class="tbl">
	<DataTable
		{columns}
		{rows}
		rowKey={(p) => p.id}
		responsive="stack"
		style="border: 0; border-radius: 0"
		loading={passkeys.isLoading}
		loadingLabel={m.common_loading()}
		empty={m.users_passkeys_empty()}
		layout="fixed"
		cellSnippets={{ label: colLabel, created: colCreated, used: colUsed, actions: colActions }}
	/>
</section>

{#if dropTarget}
	{@const p = dropTarget}
	<ConfirmModal
		open
		tone="danger"
		title={m.settings_passkeys_revoke()}
		message={m.users_confirm_revoke_passkey({ name: userName, label: p.label })}
		confirmLabel={m.settings_passkeys_revoke()}
		onconfirm={() => drop(p)}
		oncancel={() => (dropTarget = null)}
	/>
{/if}

<style>
	.tbl {
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		overflow: hidden;
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
</style>
