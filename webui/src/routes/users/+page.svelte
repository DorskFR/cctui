<script lang="ts">
	import { useUsers, useUserActions, useMe } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { dateOnly } from '$lib/format';
	import UserExpand from '$lib/components/UserExpand.svelte';
	import SecretReveal from '$lib/components/SecretReveal.svelte';
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
	<h1 class="page-title">Users</h1>
	<div class="spacer"></div>
	<button class="btn-control btn-primary" onclick={createUser}>+ New user</button>
</div>

<!-- Who am I (CCT-251): role + identity + a non-secret preview of the stored
     bearer, so "user token required" errors stop being a mystery. -->
{#if $me.data}
	{@const m = $me.data}
	<div class="card whoami row">
		<span class="faint">Signed in as</span>
		<span class="badge" class:badge-warn={m.role === 'admin'} class:badge-ok={m.role === 'user'}
			>{m.role}</span
		>
		{#if m.user_name}<span class="who-name">{m.user_name}</span>{/if}
		<span class="mono faint preview">{m.token_preview}</span>
		{#if m.role === 'admin'}
			<span class="faint sm note"
				>The admin token is server-wide and owns no machines or accounts — OAuth accounts are
				created under a user.</span
			>
		{/if}
		<div class="spacer"></div>
		<button class="btn btn-sm" onclick={() => auth.clear()}>⏻ Log out</button>
	</div>
{/if}

{#if ($users.data ?? []).length > 6}
	<input class="input filter" placeholder="Filter users…" bind:value={filter} />
{/if}

{#if $users.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else if shown.length === 0}
	<div class="empty">{filter.trim() ? 'No matching users.' : 'No users yet.'}</div>
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
									<span class="name truncate">{u.name}</span>
								</button>
								{#if !u.revoked_at}
									<button
										class="pen"
										title="Rename user"
										aria-label="Rename user"
										onclick={() => rename(u.id, u.name)}>✎</button
									>
								{/if}
							</span>
						</td>
						<td class="col-status">
							{#if u.revoked_at}
								<span class="badge badge-danger">revoked</span>
							{:else if u.disabled_at}
								<span class="badge badge-warn">disabled</span>
							{:else}
								<span class="badge badge-ok">active</span>
								{#if !u.can_dispatch}
									<span class="badge badge-warn">no dispatch</span>
								{/if}
							{/if}
						</td>
						<td class="col-created faint">{dateOnly(u.created_at)}</td>
						<td class="col-actions">
							<div class="row row-wrap acts">
								{#if u.revoked_at}
									<button class="btn btn-sm btn-danger" onclick={() => purgeUser(u.id, u.name)}>Delete</button>
								{:else}
									<button
										class="switch"
										class:on={!u.disabled_at}
										role="switch"
										aria-checked={!u.disabled_at}
										title={u.disabled_at ? 'Enable user' : 'Disable user (temporary)'}
										aria-label="Active"
										onclick={() => toggleDisabled(u.id, u.name, !u.disabled_at)}
									>
										<span class="knob"></span>
									</button>
									<button class="btn btn-sm btn-danger" onclick={() => revoke(u.id, u.name)}>Revoke</button>
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
	.page-title {
		font-size: var(--fs-2xl);
	}
	.whoami {
		gap: var(--sp-2);
		padding: var(--sp-2) var(--sp-3);
		margin-bottom: var(--sp-4);
		flex-wrap: wrap;
	}
	.who-name {
		font-weight: var(--fw-semibold);
	}
	.preview {
		font-size: var(--fs-xs);
	}
	.note {
		font-size: var(--fs-xs);
	}
	.filter {
		margin-bottom: var(--sp-3);
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
	.name {
		font-weight: var(--fw-semibold);
	}
	.pen {
		flex: none;
		background: none;
		border: none;
		padding: 0 var(--sp-1);
		cursor: pointer;
		color: var(--text-muted);
		font-size: var(--fs-sm);
		opacity: 0;
		transition: opacity 0.12s var(--ease);
	}
	.user-row:hover .pen,
	.pen:focus-visible {
		opacity: 1;
	}
	.pen:hover {
		color: var(--text);
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
	.col-status .badge + .badge {
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
	/* pill toggle (matches UserExpand's switch) */
	.switch {
		flex: none;
		width: 2.75rem;
		height: 1.6rem;
		border-radius: var(--r-pill);
		border: 1px solid var(--border-strong);
		background: var(--bg-elevated-2);
		padding: 2px;
		display: flex;
		align-items: center;
		cursor: pointer;
		transition:
			background 0.14s var(--ease),
			border-color 0.14s var(--ease);
	}
	.switch .knob {
		width: 1.25rem;
		height: 1.25rem;
		border-radius: 50%;
		background: var(--text-muted);
		transition:
			transform 0.14s var(--ease),
			background 0.14s var(--ease);
	}
	.switch.on {
		background: var(--accent);
		border-color: var(--accent);
	}
	.switch.on .knob {
		transform: translateX(1.15rem);
		background: var(--text-on-accent);
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
