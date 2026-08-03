<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import type { MachineRow } from '@bindings/MachineRow';
	import type { UserTokenRow } from '@bindings/UserTokenRow';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import {
		Badge,
		Button,
		Card,
		Cluster,
		DataTable,
		Heading,
		IconButton,
		Text,
		Timestamp
	} from '@dorsk/tsumikit';
	import type { Column } from '@dorsk/tsumikit';
	import EditEntityModal from '$lib/components/molecules/EditEntityModal.svelte';
	import { useMachines, useTokens, useUserActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	// The selected user's machines and API tokens, each in its own
	// labelled card with a DataTable — no nested master/detail tables.
	let {
		user,
		onsecret
	}: {
		user: UserRow;
		onsecret: (title: string, secret: string) => void;
	} = $props();

	const revoked = $derived(!!user.revoked_at);

	const machines = useMachines(
		() => user.id,
		() => true
	);
	const tokens = useTokens(
		() => user.id,
		() => true
	);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	// Real enrolled daemons plus the server-managed per-user `dispatch` machine
	// (shown read-only so its badge color stays editable). One-shot
	// `ephemeral` worker pods stay hidden.
	const shownMachines = $derived((machines.data ?? []).filter((m) => m.kind !== 'ephemeral'));
	const hiddenCount = $derived((machines.data ?? []).length - shownMachines.length);
	const tokenRows = $derived(tokens.data ?? []);

	// Preset hue swatches for the per-machine color override. Shown
	// in a popover anchored to the machine badge, not inline.
	const HUES = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330];

	const machineCols: Column<MachineRow>[] = [
		{ key: 'machine', label: m.users_col_machine() },
		{ key: 'status', label: m.users_col_status(), width: '8rem' },
		{ key: 'seen', label: m.users_col_last_seen(), width: '11rem' },
		{ key: 'actions', label: '', width: '7rem', align: 'right' }
	];
	const tokenCols: Column<UserTokenRow>[] = [
		{ key: 'label', label: m.users_col_token() },
		{ key: 'status', label: m.users_col_status(), width: '8rem' },
		{ key: 'created', label: m.users_col_created(), width: '13rem' },
		{ key: 'actions', label: '', width: '17rem', align: 'right' }
	];

	// Edit/create flows go through EditEntityModal — no native prompt().
	let mintOpen = $state(false);
	let editMachine = $state<MachineRow | null>(null);
	let editToken = $state<UserTokenRow | null>(null);

	function mintToken(label: string | null) {
		guard(actions.mintToken(user.id, label).then((r) => onsecret(m.users_token_secret_title({ name: user.name }), r.token)));
	}
	function relabelToken(tokenId: string, label: string | null) {
		guard(actions.relabelToken(user.id, tokenId, label));
	}
	function revokeToken(tokenId: string) {
		if (confirm(m.users_confirm_revoke_token())) guard(actions.revokeToken(user.id, tokenId));
	}
	function deleteToken(tokenId: string) {
		if (confirm(m.users_confirm_delete_token()))
			guard(actions.purgeToken(user.id, tokenId));
	}
	function saveMachine(id: string, displayName: string | null, hue: number | null) {
		guard(actions.updateMachine(user.id, id, displayName, hue));
	}
	function revokeMachine(id: string) {
		if (confirm(m.users_confirm_revoke_machine()))
			guard(actions.revokeMachine(user.id, id));
	}
	function purgeMachine(id: string) {
		if (confirm(m.users_confirm_purge_machine())) guard(actions.purgeMachine(user.id, id));
	}
</script>

{#snippet mcMachine(mc: MachineRow)}
	<Cluster gap="var(--sp-1)" wrap={false}>
		<MachineBadge name={mc.display_name || mc.name} id={mc.id} hue={mc.hue} />
		{#if !mc.revoked_at}
			<IconButton
				inline
				icon="edit"
				size={14}
				title={m.users_edit_machine()}
				label={m.users_edit_machine()}
				onclick={() => (editMachine = mc)}
			/>
		{/if}
	</Cluster>
{/snippet}
{#snippet mcStatus(mc: MachineRow)}
	{#if mc.revoked_at}
		<Badge tone="danger">{m.users_badge_revoked()}</Badge>
	{:else if mc.kind === 'dispatch'}
		<Badge tone="neutral">{m.users_badge_system()}</Badge>
	{:else}
		<Badge tone="ok">{m.users_badge_enrolled()}</Badge>
	{/if}
{/snippet}
{#snippet mcSeen(mc: MachineRow)}
	<Text size="xs" tone="faint" truncate>{#if mc.last_seen_at}{m.users_seen_prefix()} <Timestamp value={mc.last_seen_at} mode="relative" tone="inherit" />{/if}</Text>
{/snippet}
{#snippet mcActions(mc: MachineRow)}
	{#if mc.revoked_at}
		<Button variant="danger" onclick={() => purgeMachine(mc.id)}>{m.users_purge()}</Button>
	{:else if mc.kind !== 'dispatch'}
		<Button variant="danger" onclick={() => revokeMachine(mc.id)}>{m.users_revoke()}</Button>
	{/if}
{/snippet}

{#snippet tkLabel(t: UserTokenRow)}
	<div class="stack tk-id">
		<Text truncate>{t.label || m.users_unlabeled()}</Text>
		<Text size="xs" tone="faint" variant="code" truncate>{t.token_preview ?? '••••••••'}</Text>
	</div>
{/snippet}
{#snippet tkStatus(t: UserTokenRow)}
	{#if t.revoked_at}
		<Badge tone="danger">{m.users_badge_revoked()}</Badge>
	{:else}
		<Badge tone="ok">{m.users_badge_active()}</Badge>
	{/if}
{/snippet}
{#snippet tkCreated(t: UserTokenRow)}
	<Text size="xs" tone="faint" truncate>
		<Timestamp value={t.created_at} mode="date" tone="inherit" />{#if t.expires_at} {m.users_expires_prefix()} <Timestamp
				value={t.expires_at}
				mode="date"
				tone="inherit"
			/>{/if}
	</Text>
{/snippet}
{#snippet tkActions(t: UserTokenRow)}
	<div class="row mini">
		{#if t.revoked_at}
			<Button variant="danger" onclick={() => deleteToken(t.id)}>{m.common_delete()}</Button>
		{:else}
			<Button onclick={() => (editToken = t)}>{m.users_relabel()}</Button>
			<Button variant="danger" onclick={() => revokeToken(t.id)}>{m.users_revoke()}</Button>
		{/if}
	</div>
{/snippet}

<div class="stack expand">
	<!-- Machines -->
	<div class="sec-card">
		<Card>
			<div class="sec-head">
				<Heading level={3} size="sm">{m.users_machines_title()}</Heading>
				<Text as="p" size="xs" tone="faint"
					>{m.users_machines_help()}</Text
				>
			</div>
			{#if machines.isLoading}
				<span class="spin"></span>
			{:else}
				<DataTable
					columns={machineCols}
					rows={shownMachines}
					rowKey={(m) => m.id}
					empty={m.users_machines_empty()}
					cellSnippets={{ machine: mcMachine, status: mcStatus, seen: mcSeen, actions: mcActions }}
				/>
			{/if}
			{#if hiddenCount > 0}
				<Text as="p" size="xs" tone="faint"
					>{m.users_machines_hidden({ count: hiddenCount })}</Text
				>
			{/if}
		</Card>
	</div>

	<!-- API tokens -->
	<div class="sec-card">
		<Card>
			<div class="sec-head row">
				<div class="stack">
					<Heading level={3} size="sm">{m.users_tokens_title()}</Heading>
					<Text as="p" size="xs" tone="faint"
						>{m.users_tokens_help()}</Text
					>
				</div>
				<div class="spacer"></div>
				{#if !revoked}
					<Button onclick={() => (mintOpen = true)}>{m.users_new_token()}</Button>
				{/if}
			</div>
			{#if tokens.isLoading}
				<span class="spin"></span>
			{:else}
				<DataTable
					columns={tokenCols}
					rows={tokenRows}
					rowKey={(t) => t.id}
					empty={m.users_tokens_empty()}
					cellSnippets={{ label: tkLabel, status: tkStatus, created: tkCreated, actions: tkActions }}
				/>
			{/if}
		</Card>
	</div>
</div>

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
		onsave={(name, hue) => saveMachine(mc.id, name, hue)}
		onclose={() => (editMachine = null)}
	/>
{/if}

{#if editToken}
	{@const t = editToken}
	<EditEntityModal
		title={m.users_relabel_token()}
		fieldLabel={m.users_field_label()}
		name={t.label}
		placeholder={m.users_unlabeled()}
		onsave={(label) => relabelToken(t.id, label)}
		onclose={() => (editToken = null)}
	/>
{/if}

{#if mintOpen}
	<EditEntityModal
		title={m.users_new_token_title()}
		fieldLabel={m.users_field_label_optional()}
		placeholder={m.users_token_placeholder()}
		saveLabel={m.users_create_token()}
		onsave={(label) => mintToken(label)}
		onclose={() => (mintOpen = false)}
	/>
{/if}

<style>
	.expand {
		gap: var(--sp-3);
	}
	.sec-head {
		gap: var(--sp-1);
		align-items: flex-start;
		margin-bottom: var(--sp-3);
	}
	.tk-id {
		gap: 0;
		min-width: 0;
	}
	.mini {
		flex: 0 0 auto;
		gap: var(--sp-1);
		justify-content: flex-end;
	}
</style>
