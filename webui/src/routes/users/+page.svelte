<script lang="ts">
	import { useUsers, useUserActions, useMe } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly } from '$lib/format';
	import UserExpand from '$lib/components/molecules/UserExpand.svelte';
	import SecretReveal from '$lib/components/molecules/SecretReveal.svelte';
	import Button from '$lib/components/atoms/Button.svelte';
	import Input from '$lib/components/atoms/Input.svelte';
	import Badge from '$lib/components/atoms/Badge.svelte';
	import Switch from '$lib/components/atoms/Switch.svelte';
	import Heading from '$lib/components/atoms/Heading.svelte';
	import Text from '$lib/components/atoms/Text.svelte';
	import IconButton from '$lib/components/molecules/IconButton.svelte';
	import { SvelteSet } from 'svelte/reactivity';
	import { auth } from '$lib/auth.svelte';

	const users = useUsers();
	const me = useMe();
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	let secret = $state<{ title: string; value: string } | null>(null);
	// Tree state: which user rows are expanded inline (CCT-222 — no modals).
	const expanded = new SvelteSet<string>();
	let filter = $state('');

	function showSecret(title: string, value: string) {
		secret = { title, value };
	}
	function toggle(id: string) {
		if (expanded.has(id)) expanded.delete(id);
		else expanded.add(id);
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
				expanded.delete(id);
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
</script>

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
{:else if shown.length === 0}
	<div class="empty"><Text tone="muted">{filter.trim() ? 'No matching users.' : 'No users yet.'}</Text></div>
{:else}
	<div class="card table-card">
		<table class="users">
			<thead>
				<tr>
					<th class="col-name">User</th>
					<th class="col-status">Status</th>
					<th class="col-created">Created</th>
					<th class="col-actions">Actions</th>
				</tr>
			</thead>
			<tbody>
				{#each shown as u (u.id)}
					{@const open = expanded.has(u.id)}
					<tr class="user-row" class:open>
						<td class="col-name">
							<span class="row name-line">
								<button class="name-btn row" onclick={() => toggle(u.id)} aria-expanded={open}>
									<span class="caret" class:open>›</span>
									<Text weight="semibold" truncate>{u.name}</Text>
								</button>
								{#if !u.revoked_at}
									<IconButton
										inline
										class="pen"
										icon="edit"
										size={14}
										title="Rename user"
										label="Rename user"
										onclick={() => rename(u.id, u.name)}
									/>
								{/if}
							</span>
						</td>
						<td class="col-status">
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
						</td>
						<td class="col-created faint">{dateOnly(u.created_at)}</td>
						<td class="col-actions">
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
						</td>
					</tr>
					{#if open}
						<tr class="expand-row">
							<td colspan="4">
								<UserExpand user={u} onsecret={showSecret} />
							</td>
						</tr>
					{/if}
				{/each}
			</tbody>
		</table>
	</div>
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
	}
	.table-card {
		padding: 0;
		overflow-x: auto;
	}
	table.users {
		width: 100%;
		border-collapse: collapse;
		/* Fixed layout: column widths never shift when rows expand (no CLS). */
		table-layout: fixed;
	}
	th {
		text-align: left;
		font-size: var(--fs-xs);
		color: var(--text-muted);
		text-transform: uppercase;
		letter-spacing: 0.04em;
		font-weight: var(--fw-semibold);
		padding: var(--sp-2) var(--sp-3);
		border-bottom: 1px solid var(--border);
	}
	td {
		padding: var(--sp-2) var(--sp-3);
		vertical-align: middle;
	}
	.user-row td {
		border-top: 1px solid var(--border);
	}
	tbody tr:first-child td {
		border-top: none;
	}
	.col-status {
		width: 11rem;
	}
	.col-created {
		width: 8rem;
	}
	.col-actions {
		width: 12rem;
	}
	.name-line {
		gap: var(--sp-1);
		max-width: 100%;
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
	}
	.user-row :global(.pen) {
		flex: none;
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.user-row:hover :global(.pen),
	.user-row :global(.pen:focus-visible) {
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
	.col-status :global(.badge + .badge) {
		margin-left: var(--sp-1);
	}
	.acts {
		gap: var(--sp-2);
		align-items: center;
	}
	.expand-row td {
		padding: 0;
		background: var(--bg-elevated);
		border-top: 1px solid var(--border);
	}
	@media (max-width: 720px) {
		.col-created {
			display: none;
		}
		.col-status {
			width: 7rem;
		}
		.col-actions {
			width: 10rem;
		}
	}
</style>
