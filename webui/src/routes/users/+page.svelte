<script lang="ts">
	import { useUsers, useUserActions, useMe } from '$lib/queries';
	import type { UserRow } from '@bindings/UserRow';
	import { toasts } from '$lib/toast.svelte';
	import UserExpand from '$lib/components/molecules/UserExpand.svelte';
	import UserScopes from '$lib/components/molecules/UserScopes.svelte';
	import SecretReveal from '$lib/components/molecules/SecretReveal.svelte';
	import EditEntityModal from '$lib/components/molecules/EditEntityModal.svelte';
	import {
		Badge,
		Button,
		Card,
		Field,
		Heading,
		IconButton,
		Select,
		Switch,
		Text,
		Timestamp
	} from '@dorsk/tsumikit';
	import { auth } from '$lib/auth.svelte';

	const me = useMe();
	// Only an admin may list all users; a non-admin uses the self-service view.
	const users = useUsers(() => $me.data?.role === 'admin');
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	let secret = $state<{ title: string; value: string } | null>(null);
	// Pick a user from the dropdown; their detail renders full-width below.
	// No table-inside-a-table (CCT-301). Create/rename go through a modal, not
	// native prompt() dialogs.
	let selectedId = $state('');
	let createOpen = $state(false);
	let renameUser = $state<UserRow | null>(null);

	function showSecret(title: string, value: string) {
		secret = { title, value };
	}

	async function createUser(name: string | null) {
		if (!name) return;
		try {
			const r = await actions.create(name);
			selectedId = r.id;
			showSecret(`Key — ${r.name}`, r.key);
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}
	function rename(id: string, name: string | null) {
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
				if (selectedId === id) selectedId = '';
			})
		);
	}

	const all = $derived($users.data ?? []);
	const active = $derived(all.filter((u) => !u.revoked_at));
	const revoked = $derived(all.filter((u) => !!u.revoked_at));
	const selected = $derived(all.find((u) => u.id === selectedId) ?? null);
</script>

<div class="bar row">
	<Heading level={1}>Users</Heading>
	<div class="spacer"></div>
	{#if $me.data?.role === 'admin'}
		<Button control variant="primary" onclick={() => (createOpen = true)}>+ New user</Button>
	{/if}
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
		<Button onclick={() => auth.clear()}>⏻ Log out</Button>
	</div>
{/if}

{#if $me.data && $me.data.role !== 'admin'}
	<!-- Self-service (CCT-410): a non-admin user manages its own keys here (mint
	     a dispatch-only key for automation, etc.) without an admin token. The ceiling is
	     read-only for them; only an admin can grant new capabilities. -->
	{#if $me.data.user_id}
		<div class="detail-card">
			<UserScopes userId={$me.data.user_id} isAdmin={false} isSelf={true} onsecret={showSecret} />
		</div>
	{/if}
{:else if $users.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else}
	<div class="picker">
		<Field label="Select a user" for="user-select">
			<Select id="user-select" bind:value={selectedId}>
				<option value="">Select a user…</option>
				<optgroup label="Active">
					{#each active as u (u.id)}<option value={u.id}>{u.name}</option>{/each}
				</optgroup>
				{#if revoked.length}
					<optgroup label="Revoked">
						{#each revoked as u (u.id)}<option value={u.id}>{u.name}</option>{/each}
					</optgroup>
				{/if}
			</Select>
		</Field>
	</div>

	{#if selected}
		{@const u = selected}
		<div class="detail-card">
			<Card>
				<header class="head">
					<Heading level={2}>{u.name}</Heading>
					{#if u.revoked_at}
						<Badge tone="danger">revoked</Badge>
					{:else if u.disabled_at}
						<Badge tone="warn">disabled</Badge>
					{:else}
						<Badge tone="ok">active</Badge>
					{/if}
					{#if !u.revoked_at}
						<IconButton
							inline
							icon="edit"
							size={14}
							title="Rename user"
							label="Rename user"
							onclick={() => (renameUser = u)}
						/>
					{/if}
				</header>

				<dl class="props">
					<div class="prop">
						<dt><Text size="sm" tone="faint">Created</Text></dt>
						<dd><Timestamp value={u.created_at} mode="date" size="sm" tone="inherit" /></dd>
					</div>
					<div class="prop">
						<dt><Text size="sm" tone="faint">Active</Text></dt>
						<dd>
							<Switch
								checked={!u.disabled_at}
								label="Active"
								title={u.disabled_at ? 'Enable user' : 'Disable user (temporary)'}
								disabled={!!u.revoked_at}
								onclick={() => toggleDisabled(u.id, u.name, !u.disabled_at)}
							/>
						</dd>
					</div>
				</dl>
					<Text size="xs" tone="faint"
						>Dispatch permission is now the <Text variant="code">dispatch</Text> scope below.</Text
					>

				<footer class="acts">
					{#if u.revoked_at}
						<Button variant="danger" onclick={() => purgeUser(u.id, u.name)}
							>Delete permanently</Button
						>
					{:else}
						<Button variant="danger" onclick={() => revoke(u.id, u.name)}>Revoke</Button>
					{/if}
				</footer>
			</Card>
		</div>

		<div class="detail-card">
			<UserScopes
				userId={u.id}
				isAdmin={$me.data?.role === 'admin'}
				isSelf={$me.data?.user_id === u.id}
				onsecret={showSecret}
			/>
		</div>

		<UserExpand user={selected} onsecret={showSecret} />
	{/if}
{/if}

{#if createOpen}
	<EditEntityModal
		title="New user"
		fieldLabel="User name"
		placeholder="e.g. alice"
		saveLabel="Create user"
		onsave={(name) => createUser(name)}
		onclose={() => (createOpen = false)}
	/>
{/if}

{#if renameUser}
	{@const u = renameUser}
	<EditEntityModal
		title="Rename user"
		fieldLabel="User name"
		name={u.name}
		onsave={(name) => rename(u.id, name)}
		onclose={() => (renameUser = null)}
	/>
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
	/* Select spans the full width — it's the primary navigation control. */
	.picker {
		margin-bottom: var(--sp-4);
	}
	/* The detail card stays at its natural width (full-width only on narrow
	   screens via max-width), like the accounts cards — it holds little info. */
	.detail-card {
		max-width: 30rem;
		margin-bottom: var(--sp-4);
	}
	.head {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
		flex-wrap: wrap;
		padding-bottom: var(--sp-3);
		border-bottom: 1px solid var(--border);
	}
	.props {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: var(--sp-3) var(--sp-4);
		align-items: center;
		margin: var(--sp-3) 0;
	}
	.prop {
		display: contents;
	}
	.props dt {
		margin: 0;
	}
	.props dd {
		margin: 0;
		display: flex;
		align-items: center;
		justify-content: flex-end;
	}
	.acts {
		display: flex;
		justify-content: flex-end;
		gap: var(--sp-2);
		padding-top: var(--sp-3);
		border-top: 1px solid var(--border);
	}
</style>
