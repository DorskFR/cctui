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

	// The selected user's machines and API tokens (CCT-301), each in its own
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
	// (shown read-only so its badge color stays editable — CCT-251). One-shot
	// `ephemeral` worker pods stay hidden.
	const shownMachines = $derived(($machines.data ?? []).filter((m) => m.kind !== 'ephemeral'));
	const hiddenCount = $derived(($machines.data ?? []).length - shownMachines.length);
	const tokenRows = $derived($tokens.data ?? []);

	// Preset hue swatches for the per-machine color override (CCT-222). Shown
	// in a popover anchored to the machine badge (CCT-251), not inline.
	const HUES = [0, 30, 60, 90, 120, 150, 180, 210, 240, 270, 300, 330];

	const machineCols: Column<MachineRow>[] = [
		{ key: 'machine', label: 'Machine' },
		{ key: 'status', label: 'Status', width: '8rem' },
		{ key: 'seen', label: 'Last seen', width: '11rem' },
		{ key: 'actions', label: '', width: '7rem', align: 'right' }
	];
	const tokenCols: Column<UserTokenRow>[] = [
		{ key: 'label', label: 'Token' },
		{ key: 'status', label: 'Status', width: '8rem' },
		{ key: 'created', label: 'Created', width: '13rem' },
		{ key: 'actions', label: '', width: '17rem', align: 'right' }
	];

	// Edit/create flows go through EditEntityModal (CCT-301) — no native prompt().
	let mintOpen = $state(false);
	let editMachine = $state<MachineRow | null>(null);
	let editToken = $state<UserTokenRow | null>(null);

	function mintToken(label: string | null) {
		guard(actions.mintToken(user.id, label).then((r) => onsecret(`Token — ${user.name}`, r.token)));
	}
	function relabelToken(tokenId: string, label: string | null) {
		guard(actions.relabelToken(user.id, tokenId, label));
	}
	function revokeToken(tokenId: string) {
		if (confirm('Revoke this token?')) guard(actions.revokeToken(user.id, tokenId));
	}
	function deleteToken(tokenId: string) {
		if (confirm('Delete this token? It is revoked and removed in one step.'))
			guard(actions.purgeToken(user.id, tokenId));
	}
	function saveMachine(id: string, displayName: string | null, hue: number | null) {
		guard(actions.updateMachine(user.id, id, displayName, hue));
	}
	function revokeMachine(id: string) {
		if (confirm('Revoke this machine? Its key stops working; the daemon must re-enroll.'))
			guard(actions.revokeMachine(user.id, id));
	}
	function purgeMachine(id: string) {
		if (confirm('Permanently remove this revoked machine?')) guard(actions.purgeMachine(user.id, id));
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
				title="Edit machine"
				label="Edit machine"
				onclick={() => (editMachine = mc)}
			/>
		{/if}
	</Cluster>
{/snippet}
{#snippet mcStatus(mc: MachineRow)}
	{#if mc.revoked_at}
		<Badge tone="danger">revoked</Badge>
	{:else if mc.kind === 'dispatch'}
		<Badge tone="neutral">system</Badge>
	{:else}
		<Badge tone="ok">enrolled</Badge>
	{/if}
{/snippet}
{#snippet mcSeen(mc: MachineRow)}
	<Text size="xs" tone="faint" truncate>{#if mc.last_seen_at}seen <Timestamp value={mc.last_seen_at} mode="relative" tone="inherit" />{/if}</Text>
{/snippet}
{#snippet mcActions(mc: MachineRow)}
	{#if mc.revoked_at}
		<Button variant="danger" onclick={() => purgeMachine(mc.id)}>Purge</Button>
	{:else if mc.kind !== 'dispatch'}
		<Button variant="danger" onclick={() => revokeMachine(mc.id)}>Revoke</Button>
	{/if}
{/snippet}

{#snippet tkLabel(t: UserTokenRow)}
	<div class="stack tk-id">
		<Text truncate>{t.label || '(unlabeled)'}</Text>
		<Text size="xs" tone="faint" variant="code" truncate>{t.token_preview ?? '••••••••'}</Text>
	</div>
{/snippet}
{#snippet tkStatus(t: UserTokenRow)}
	{#if t.revoked_at}
		<Badge tone="danger">revoked</Badge>
	{:else}
		<Badge tone="ok">active</Badge>
	{/if}
{/snippet}
{#snippet tkCreated(t: UserTokenRow)}
	<Text size="xs" tone="faint" truncate>
		<Timestamp value={t.created_at} mode="date" tone="inherit" />{#if t.expires_at} · expires <Timestamp
				value={t.expires_at}
				mode="date"
				tone="inherit"
			/>{/if}
	</Text>
{/snippet}
{#snippet tkActions(t: UserTokenRow)}
	<div class="row mini">
		{#if t.revoked_at}
			<Button variant="danger" onclick={() => deleteToken(t.id)}>Delete</Button>
		{:else}
			<Button onclick={() => (editToken = t)}>Relabel</Button>
			<Button variant="danger" onclick={() => revokeToken(t.id)}>Revoke</Button>
		{/if}
	</div>
{/snippet}

<div class="stack expand">
	<!-- Machines -->
	<div class="sec-card">
		<Card>
			<div class="sec-head">
				<Heading level={3} size="sm">Machines</Heading>
				<Text as="p" size="xs" tone="faint"
					>Daemons enrolled to this user — each connects with its own machine key.</Text
				>
			</div>
			{#if $machines.isLoading}
				<span class="spin"></span>
			{:else}
				<DataTable
					columns={machineCols}
					rows={shownMachines}
					rowKey={(m) => m.id}
					empty="No machines enrolled."
					cellSnippets={{ machine: mcMachine, status: mcStatus, seen: mcSeen, actions: mcActions }}
				/>
			{/if}
			{#if hiddenCount > 0}
				<Text as="p" size="xs" tone="faint"
					>{hiddenCount} ephemeral worker machine{hiddenCount === 1 ? '' : 's'} hidden.</Text
				>
			{/if}
		</Card>
	</div>

	<!-- API tokens -->
	<div class="sec-card">
		<Card>
			<div class="sec-head row">
				<div class="stack">
					<Heading level={3} size="sm">API tokens</Heading>
					<Text as="p" size="xs" tone="faint"
						>Bearer tokens that authenticate API and CLI requests as this user.</Text
					>
				</div>
				<div class="spacer"></div>
				{#if !revoked}
					<Button onclick={() => (mintOpen = true)}>+ New token</Button>
				{/if}
			</div>
			{#if $tokens.isLoading}
				<span class="spin"></span>
			{:else}
				<DataTable
					columns={tokenCols}
					rows={tokenRows}
					rowKey={(t) => t.id}
					empty="No tokens."
					cellSnippets={{ label: tkLabel, status: tkStatus, created: tkCreated, actions: tkActions }}
				/>
			{/if}
		</Card>
	</div>
</div>

{#if editMachine}
	{@const mc = editMachine}
	<EditEntityModal
		title="Edit machine"
		fieldLabel="Display name"
		name={mc.display_name}
		placeholder={mc.name}
		hint="Leave blank to use the machine's reported hostname."
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
		title="Relabel token"
		fieldLabel="Label"
		name={t.label}
		placeholder="(unlabeled)"
		onsave={(label) => relabelToken(t.id, label)}
		onclose={() => (editToken = null)}
	/>
{/if}

{#if mintOpen}
	<EditEntityModal
		title="New API token"
		fieldLabel="Label (optional)"
		placeholder="e.g. laptop CLI"
		saveLabel="Create token"
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
