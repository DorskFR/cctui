<script lang="ts">
	import type { MachineRow } from '@bindings/MachineRow';
	import { Button, ConfirmModal, Dot, IconButton, Text, Timestamp } from '@dorsk/tsumikit';
	import AccessTable, { type AccessColumn } from '$lib/components/molecules/AccessTable.svelte';
	import EditEntityModal from '$lib/components/molecules/EditEntityModal.svelte';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import RowActions from '$lib/components/molecules/RowActions.svelte';
	import { useMachines, useUserActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';
	import { splitRevoked, visibleRows } from './access.logic';

	let { userId, canManage }: { userId: string; canManage: boolean } = $props();

	const HUES = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330];

	const machines = useMachines(
		() => userId,
		() => true
	);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: unknown) => toasts.error(errMessage(e)));

	let showRevoked = $state(false);
	let editMachine = $state<MachineRow | null>(null);
	let dropTarget = $state<MachineRow | null>(null);

	const shown = $derived((machines.data ?? []).filter((mc) => mc.kind !== 'ephemeral'));
	const hiddenCount = $derived((machines.data ?? []).length - shown.length);
	const groups = $derived(splitRevoked(shown));
	const rows = $derived(visibleRows(shown, showRevoked));

	const columns: AccessColumn[] = [
		{ key: 'machine', label: m.users_col_machine(), width: 'minmax(0, 1.2fr)' },
		{ key: 'status', label: m.users_col_status(), width: 'minmax(0, 0.9fr)' },
		{ key: 'seen', label: m.users_col_last_seen(), width: 'minmax(0, 0.9fr)' },
		{ key: 'preview', label: m.access_col_preview(), width: 'minmax(0, 1.2fr)' },
		{ key: 'kind', label: m.access_col_kind(), width: '80px' },
		{ key: 'actions', width: '56px' }
	];

	const liveText = (mc: MachineRow) =>
		mc.revoked_at
			? m.users_badge_revoked()
			: mc.liveness === 'online'
				? m.dispatch_liveness_online()
				: mc.liveness === 'stale'
					? m.dispatch_liveness_stale()
					: m.dispatch_liveness_offline();
	const liveDot = (mc: MachineRow) =>
		mc.revoked_at || mc.liveness === 'offline'
			? 'dead'
			: mc.liveness === 'stale'
				? 'stale'
				: 'active';

	function save(mc: MachineRow, displayName: string | null, hue: number | null) {
		guard(actions.updateMachine(userId, mc.id, displayName, hue));
	}
	function drop(mc: MachineRow) {
		guard(mc.revoked_at ? actions.purgeMachine(userId, mc.id) : actions.revokeMachine(userId, mc.id));
		dropTarget = null;
	}
</script>

<AccessTable
	{columns}
	{rows}
	rowKey={(mc) => mc.id}
	loading={machines.isLoading}
	empty={m.users_machines_empty()}
	dim={(mc) => !!mc.revoked_at}
>
	{#snippet bar()}
		<Text size="xs" tone="faint"
			>{m.access_counts({ active: groups.active.length, revoked: groups.revoked.length })}</Text
		>
		<div class="spacer"></div>
		{#if hiddenCount > 0}
			<Text size="xs" tone="faint">{m.users_machines_hidden({ count: hiddenCount })}</Text>
		{/if}
		{#if groups.revoked.length}
			<Button variant="link" size="sm" onclick={() => (showRevoked = !showRevoked)}>
				{showRevoked ? m.access_hide_revoked() : m.access_show_revoked()}
			</Button>
		{/if}
	{/snippet}

	{#snippet row(mc: MachineRow)}
		<span class="lead">
			<MachineBadge name={mc.display_name || mc.name} id={mc.id} hue={mc.hue} />
		</span>
		<span class="live">
			<Dot status={liveDot(mc)} />
			<Text size="xs" tone="muted">{liveText(mc)}</Text>
		</span>
		<span class="stamp">
			<Timestamp value={mc.last_seen_at} mode="relative" size="xs" tone="faint" details={false} />
		</span>
		<span class="mono faint">{mc.key_preview ?? '••••'}</span>
		<span class="kind"><Text size="xs" tone="faint">{mc.kind}</Text></span>
		{#if canManage && mc.kind !== 'dispatch'}
			<RowActions>
				{#if !mc.revoked_at}
					<IconButton
						inline
						icon="edit"
						size={14}
						label={m.users_edit_machine()}
						title={m.users_edit_machine()}
						onclick={() => (editMachine = mc)}
					/>
				{/if}
				<IconButton
					inline
					hoverDanger
					icon="trash"
					size={14}
					label={mc.revoked_at ? m.users_purge() : m.users_revoke()}
					title={mc.revoked_at ? m.users_purge() : m.users_revoke()}
					onclick={() => (dropTarget = mc)}
				/>
			</RowActions>
		{:else}
			<span></span>
		{/if}
	{/snippet}
</AccessTable>

{#if editMachine}
	{@const mc = editMachine}
	<EditEntityModal
		title={m.users_edit_machine()}
		fieldLabel={m.users_field_display_name()}
		name={mc.display_name}
		placeholder={mc.name}
		hint={m.users_machine_hint()}
		color
		hue={mc.hue}
		hues={HUES}
		onsave={(name, hue) => save(mc, name, hue)}
		onclose={() => (editMachine = null)}
	/>
{/if}

{#if dropTarget}
	{@const mc = dropTarget}
	<ConfirmModal
		open
		tone="danger"
		title={mc.revoked_at ? m.users_purge() : m.users_revoke()}
		message={mc.revoked_at ? m.users_confirm_purge_machine() : m.users_confirm_revoke_machine()}
		confirmLabel={mc.revoked_at ? m.users_purge() : m.users_revoke()}
		onconfirm={() => drop(mc)}
		oncancel={() => (dropTarget = null)}
	/>
{/if}

<style>
	.spacer {
		flex: 1;
	}
	.lead {
		min-width: 0;
		display: flex;
		align-items: center;
	}
	.live {
		display: flex;
		align-items: center;
		gap: var(--sp-1);
		min-width: 0;
	}
	.stamp,
	.kind {
		min-width: 0;
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
