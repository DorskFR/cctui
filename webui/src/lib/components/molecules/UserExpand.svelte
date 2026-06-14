<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import type { MachineRow } from '@bindings/MachineRow';
	import type { UserTokenRow } from '@bindings/UserTokenRow';
	import MachineBadge from '$lib/components/molecules/MachineBadge.svelte';
	import { Badge, Button, DataTable, Heading, IconButton, Switch, Text } from '@dorsk/tsumikit';
	import type { Column } from '@dorsk/tsumikit';
	import ColorPicker from '$lib/components/molecules/ColorPicker.svelte';
	import { useMachines, useTokens, useUserActions } from '$lib/queries';
	import { dateOnly, relativeTime } from '$lib/format';
	import { toasts } from '$lib/toast.svelte';

	// Inline expansion of a user row in the users table (CCT-222) — replaces
	// the old UserDetail modal sheet so nothing jumps or overlays. Machines and
	// tokens render as tsumikit DataTables (CCT-301) instead of hand-rolled rows.
	let {
		user,
		onsecret
	}: {
		user: UserRow;
		onsecret: (title: string, secret: string) => void;
	} = $props();

	const revoked = $derived(!!user.revoked_at);

	// Only mounted while expanded, so fetch eagerly.
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
		{ key: 'key', label: 'Key' },
		{ key: 'seen', label: 'Last seen', width: '12rem' },
		{ key: 'actions', label: '', width: '8rem', align: 'right' }
	];
	const tokenCols: Column<UserTokenRow>[] = [
		{ key: 'label', label: 'Label' },
		{ key: 'key', label: 'Key' },
		{ key: 'created', label: 'Created', width: '14rem' },
		{ key: 'actions', label: '', width: '16rem', align: 'right' }
	];

	function toggleDispatch() {
		const next = !user.can_dispatch;
		guard(
			actions
				.setCanDispatch(user.id, next)
				.then(() => toasts.ok(next ? 'Dispatch enabled' : 'Dispatch disabled'))
		);
	}
	function mintToken() {
		const label = prompt('Token label (optional)', '')?.trim() || null;
		guard(
			actions.mintToken(user.id, label).then((r) => onsecret(`Token — ${user.name}`, r.token))
		);
	}
	function relabelToken(tokenId: string, current: string | null) {
		const label = prompt('Token label', current ?? '')?.trim() || null;
		guard(actions.relabelToken(user.id, tokenId, label));
	}
	function revokeToken(tokenId: string) {
		if (confirm('Revoke this token?')) guard(actions.revokeToken(user.id, tokenId));
	}
	function deleteToken(tokenId: string) {
		if (confirm('Delete this token? It is revoked and removed in one step.'))
			guard(actions.purgeToken(user.id, tokenId));
	}
	function renameMachine(id: string, current: string | null, hue: number | null) {
		const displayName = prompt('Machine display name', current ?? '')?.trim() || null;
		guard(actions.updateMachine(user.id, id, displayName, hue));
	}
	function setHue(id: string, displayName: string | null, hue: number | null) {
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
	{@const system = mc.kind === 'dispatch'}
	<span class="row badge-line">
		<!-- Clicking the badge opens the color popover (CCT-251). -->
		<ColorPicker
			value={mc.hue}
			hues={HUES}
			label="Badge color"
			disabled={!!mc.revoked_at}
			onchange={(h) => setHue(mc.id, mc.display_name, h)}
		>
			{#snippet trigger()}
				<MachineBadge name={mc.display_name || mc.name} id={mc.id} hue={mc.hue} />
			{/snippet}
		</ColorPicker>
		{#if !mc.revoked_at && !system}
			<IconButton
				inline
				icon="edit"
				size={14}
				title="Rename machine"
				label="Rename machine"
				onclick={() => renameMachine(mc.id, mc.display_name, mc.hue)}
			/>
		{/if}
		{#if system}<Badge>dispatch</Badge>{/if}
	</span>
{/snippet}
{#snippet mcKey(mc: MachineRow)}
	<Text size="xs" tone="faint" variant="code" truncate>{mc.key_preview ?? '••••••••'}</Text>
{/snippet}
{#snippet mcSeen(mc: MachineRow)}
	<Text size="xs" tone="faint" truncate
		>{mc.kind === 'dispatch' ? 'server-managed · ' : ''}seen {relativeTime(mc.last_seen_at)}</Text
	>
{/snippet}
{#snippet mcActions(mc: MachineRow)}
	<div class="row row-wrap mini">
		{#if mc.revoked_at}
			<Badge tone="danger">revoked</Badge>
			<Button size="sm" variant="danger" onclick={() => purgeMachine(mc.id)}>Purge</Button>
		{:else if mc.kind !== 'dispatch'}
			<Button size="sm" variant="danger" onclick={() => revokeMachine(mc.id)}>Revoke</Button>
		{/if}
	</div>
{/snippet}

{#snippet tkLabel(t: UserTokenRow)}
	<Text truncate>{t.label || '(unlabeled)'}</Text>
{/snippet}
{#snippet tkKey(t: UserTokenRow)}
	<Text size="xs" tone="faint" variant="code" truncate>{t.token_preview ?? '••••••••'}</Text>
{/snippet}
{#snippet tkCreated(t: UserTokenRow)}
	<Text size="xs" tone="faint" truncate>
		{dateOnly(t.created_at)}{t.expires_at ? ` · expires ${dateOnly(t.expires_at)}` : ''}
	</Text>
{/snippet}
{#snippet tkActions(t: UserTokenRow)}
	<div class="row row-wrap mini">
		{#if t.revoked_at}
			<Badge tone="danger">revoked</Badge>
			<Button size="sm" variant="danger" onclick={() => deleteToken(t.id)}>Delete</Button>
		{:else}
			<Button size="sm" onclick={() => relabelToken(t.id, t.label)}>Relabel</Button>
			<Button size="sm" variant="danger" onclick={() => revokeToken(t.id)}>Revoke</Button>
			<Button size="sm" variant="danger" onclick={() => deleteToken(t.id)}>Delete</Button>
		{/if}
	</div>
{/snippet}

<div class="stack expand">
	<!-- Permissions (CCT-185) -->
	<section class="stack sec">
		<div class="sub-h"><Heading level={3} size="sm" tone="muted">Permissions</Heading></div>
		<div class="row perm">
			<div class="stack info">
				<Text>Can dispatch</Text>
				<Text size="xs" tone="faint">Allow this user to dispatch k8s worker sessions.</Text>
			</div>
			<Switch
				checked={user.can_dispatch}
				label="Can dispatch"
				disabled={revoked}
				onclick={toggleDispatch}
			/>
		</div>
	</section>

	<!-- Machines -->
	<section class="stack sec">
		<div class="sub-h"><Heading level={3} size="sm" tone="muted">Machines</Heading></div>
		{#if $machines.isLoading}<span class="spin"></span>
		{:else}
			<DataTable
				columns={machineCols}
				rows={shownMachines}
				rowKey={(m) => m.id}
				empty="No machines."
				cellSnippets={{ machine: mcMachine, key: mcKey, seen: mcSeen, actions: mcActions }}
			/>
		{/if}
		{#if hiddenCount > 0}
			<Text as="p" size="xs" tone="faint"
				>{hiddenCount} ephemeral worker machine{hiddenCount === 1 ? '' : 's'} hidden.</Text
			>
		{/if}
	</section>

	<!-- Tokens: many per user, all resolving to this same user. Minting lives
	     here (not on the user row) so it's clear what a "Token" is (CCT-251). -->
	<section class="stack sec">
		<div class="row sec-head">
			<div class="sub-h"><Heading level={3} size="sm" tone="muted">Tokens</Heading></div>
			<div class="spacer"></div>
			{#if !revoked}
				<Button size="sm" onclick={mintToken}>+ New token</Button>
			{/if}
		</div>
		{#if $tokens.isLoading}<span class="spin"></span>
		{:else}
			<DataTable
				columns={tokenCols}
				rows={tokenRows}
				rowKey={(t) => t.id}
				empty="No tokens."
				cellSnippets={{ label: tkLabel, key: tkKey, created: tkCreated, actions: tkActions }}
			/>
		{/if}
	</section>
</div>

<style>
	.expand {
		gap: var(--sp-3);
		padding: var(--sp-3) var(--sp-3) var(--sp-3) var(--sp-5);
	}
	.sec {
		gap: var(--sp-2);
	}
	.sec + .sec {
		padding-top: var(--sp-2);
		border-top: 1px solid var(--border);
	}
	.sec-head {
		gap: var(--sp-2);
	}
	/* Heading owns the size/colour; the wrapper adds the section-label chrome,
	   which inherits down into the Heading's text (no atom reach-in needed). */
	.sub-h {
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.info {
		flex: 1 1 auto;
		min-width: 0;
		gap: 0;
		overflow: hidden;
	}
	.badge-line {
		gap: var(--sp-1);
		position: relative;
		flex-wrap: wrap;
		min-width: 0;
		align-items: center;
	}
	.mini {
		flex: 0 0 auto;
		gap: var(--sp-1);
		justify-content: flex-end;
	}
	.perm {
		gap: var(--sp-3);
	}
</style>
