<script lang="ts">
	import { useUsers, useUserActions, useMe } from '$lib/queries';
	import type { UserRow } from '@bindings/UserRow';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly } from '$lib/format';
	import UserExpand from '$lib/components/molecules/UserExpand.svelte';
	import SecretReveal from '$lib/components/molecules/SecretReveal.svelte';
	import { Badge, Button, DataTable, Heading, IconButton, Input, Switch, Text } from '@dorsk/tsumikit';
	import type { Column } from '@dorsk/tsumikit';
	import { auth } from '$lib/auth.svelte';

	const users = useUsers();
	const me = useMe();
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	let secret = $state<{ title: string; value: string } | null>(null);
	// Master/detail (CCT-222 — no modals): clicking a row's caret selects it and
	// reveals its detail panel below the table. DataTable renders flat rows, so
	// the expansion lives under the table rather than interleaved.
	let selectedId = $state<string | null>(null);
	let filter = $state('');

	function showSecret(title: string, value: string) {
		secret = { title, value };
	}
	function toggle(id: string) {
		selectedId = selectedId === id ? null : id;
	}

	async function createUser() {
		const name = prompt('New user name')?.trim();
		if (!name) return;
		try {
			const r = await actions.create(name);
			showSecret(`Key — ${r.name}`, r.key);
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}
	function rename(id: string, current: string) {
		const name = prompt('New user name', current)?.trim();
		if (name) guard(actions.rename(id, name).then(() => toasts.ok('Renamed')));
	}
	function revoke(id: string, name: string) {
		if (
			!confirm(
				`Revoke ${name}? ALL their user tokens and machine keys stop working permanently. ` +
					`To turn a user off temporarily, use the active toggle instead.`
			)
		)
			return;
		guard(actions.revoke(id).then(() => toasts.ok('Revoked')));
	}
	// Non-destructive on/off (CCT-251): auth fails while disabled, nothing is
	// invalidated, flipping back restores everything.
	function toggleDisabled(id: string, name: string, disabled: boolean) {
		guard(
			actions
				.setDisabled(id, disabled)
				.then(() => toasts.ok(disabled ? `${name} disabled` : `${name} enabled`))
		);
	}
	function purgeUser(id: string, name: string) {
		if (!confirm(`Permanently delete ${name}? This cannot be undone.`)) return;
		guard(
			actions.purgeUser(id).then(() => {
				toasts.ok('User deleted');
				if (selectedId === id) selectedId = null;
			})
		);
	}

	// Active first, then revoked; within each, by creation order (stable).
	const sorted = $derived(
		[...($users.data ?? [])].sort((a, b) => Number(!!a.revoked_at) - Number(!!b.revoked_at))
	);
	const shown = $derived(
		filter.trim()
			? sorted.filter((u) => u.name.toLowerCase().includes(filter.trim().toLowerCase()))
			: sorted
	);
	const selected = $derived(shown.find((u) => u.id === selectedId) ?? null);

	const cols: Column<UserRow>[] = [
		{ key: 'name', label: 'User', sortable: true, get: (u) => u.name },
		{ key: 'status', label: 'Status', width: '12rem' },
		{ key: 'created', label: 'Created', width: '8rem', sortable: true, get: (u) => u.created_at },
		{ key: 'actions', label: 'Actions', width: '12rem', align: 'right' }
	];
</script>

{#snippet cellName(u: UserRow)}
	{@const open = selectedId === u.id}
	<span class="row name-line">
		<button class="name-btn row" onclick={() => toggle(u.id)} aria-expanded={open}>
			<span class="caret" class:open>›</span>
			<Text weight="semibold" truncate>{u.name}</Text>
		</button>
		{#if !u.revoked_at}
			<span class="pen-wrap">
				<IconButton
					inline
					icon="edit"
					size={14}
					title="Rename user"
					label="Rename user"
					onclick={() => rename(u.id, u.name)}
				/>
			</span>
		{/if}
	</span>
{/snippet}
{#snippet cellStatus(u: UserRow)}
	{#if u.revoked_at}
		<Badge tone="danger">revoked</Badge>
	{:else if u.disabled_at}
		<Badge tone="warn">disabled</Badge>
	{:else}
		<Badge tone="ok">active</Badge>
		{#if !u.can_dispatch}
			<Badge tone="warn">no dispatch</Badge>
		{/if}
	{/if}
{/snippet}
{#snippet cellCreated(u: UserRow)}
	<Text size="sm" tone="faint">{dateOnly(u.created_at)}</Text>
{/snippet}
{#snippet cellActions(u: UserRow)}
	<div class="row row-wrap acts">
		{#if u.revoked_at}
			<Button size="sm" variant="danger" onclick={() => purgeUser(u.id, u.name)}>Delete</Button>
		{:else}
			<Switch
				checked={!u.disabled_at}
				title={u.disabled_at ? 'Enable user' : 'Disable user (temporary)'}
				label="Active"
				onclick={() => toggleDisabled(u.id, u.name, !u.disabled_at)}
			/>
			<Button size="sm" variant="danger" onclick={() => revoke(u.id, u.name)}>Revoke</Button>
		{/if}
	</div>
{/snippet}

<div class="bar row">
	<Heading level={1}>Users</Heading>
	<div class="spacer"></div>
	<Button control variant="primary" onclick={createUser}>+ New user</Button>
</div>

<!-- Who am I (CCT-251): role + identity + a non-secret preview of the stored
     bearer, so "user token required" errors stop being a mystery. -->
{#if $me.data}
	{@const m = $me.data}
	<div class="card whoami row">
		<Text tone="faint">Signed in as</Text>
		<Badge tone={m.role === 'admin' ? 'warn' : m.role === 'user' ? 'ok' : 'neutral'}>{m.role}</Badge>
		{#if m.user_name}<Text weight="semibold">{m.user_name}</Text>{/if}
		<Text variant="code" tone="faint" size="xs">{m.token_preview}</Text>
		{#if m.role === 'admin'}
			<Text tone="faint" size="xs"
				>The admin token is server-wide and owns no machines or accounts — OAuth accounts are
				created under a user.</Text
			>
		{/if}
		<div class="spacer"></div>
		<Button size="sm" onclick={() => auth.clear()}>⏻ Log out</Button>
	</div>
{/if}

{#if ($users.data ?? []).length > 6}
	<Input placeholder="Filter users…" bind:value={filter} style="margin-bottom: var(--sp-3)" />
{/if}

{#if $users.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else}
	<DataTable
		columns={cols}
		rows={shown}
		rowKey={(u) => u.id}
		empty={filter.trim() ? 'No matching users.' : 'No users yet.'}
		stickyHeader
		cellSnippets={{ name: cellName, status: cellStatus, created: cellCreated, actions: cellActions }}
	/>
	{#if selected}
		<div class="card detail">
			<UserExpand user={selected} onsecret={showSecret} />
		</div>
	{/if}
{/if}

{#if secret}
	<SecretReveal title={secret.title} secret={secret.value} onclose={() => (secret = null)} />
{/if}

<style>
	.bar {
		margin-bottom: var(--sp-4);
	}
	.whoami {
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		margin-bottom: var(--sp-4);
		flex-wrap: wrap;
		align-items: center;
	}
	.detail {
		margin-top: var(--sp-3);
		padding: 0;
	}
	.name-line {
		gap: var(--sp-1);
		max-width: 100%;
		align-items: center;
	}
	.name-btn {
		background: none;
		border: none;
		padding: 0;
		gap: var(--sp-2);
		cursor: pointer;
		color: var(--text);
		font: inherit;
		min-width: 0;
		align-items: center;
	}
	.pen-wrap {
		display: inline-flex;
		flex: none;
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.name-line:hover .pen-wrap,
	.pen-wrap:focus-within {
		opacity: 1;
	}
	.caret {
		flex: none;
		color: var(--text-muted);
		font-size: var(--fs-lg);
		transition: transform 0.12s var(--ease);
	}
	.caret.open {
		transform: rotate(90deg);
	}
	.acts {
		gap: var(--sp-2);
		align-items: center;
		justify-content: flex-end;
	}
</style>
