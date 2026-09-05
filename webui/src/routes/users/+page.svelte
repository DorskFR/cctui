<script lang="ts">
	import { errMessage } from '$lib/api';
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
	import { m } from '$lib/paraglide/messages';

	const me = useMe();
	// Only an admin may list all users; a non-admin uses the self-service view.
	const users = useUsers(() => me.data?.role === 'admin');
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.error(e.message));

	let secret = $state<{ title: string; value: string } | null>(null);
	// Pick a user from the dropdown; their detail renders full-width below.
	// No table-inside-a-table. Create/rename go through a modal, not
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
			showSecret(m.users_key_secret_title({ label: r.name }), r.key);
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}
	function rename(id: string, name: string | null) {
		if (name) guard(actions.rename(id, name).then(() => toasts.ok(m.users_renamed())));
	}
	function revoke(id: string, name: string) {
		if (
			!confirm(m.users_confirm_revoke_user({ name }))
		)
			return;
		guard(actions.revoke(id).then(() => toasts.ok(m.users_revoked())));
	}
	// Non-destructive on/off: auth fails while disabled, nothing is
	// invalidated, flipping back restores everything.
	function toggleDisabled(id: string, name: string, disabled: boolean) {
		guard(
			actions
				.setDisabled(id, disabled)
				.then(() => toasts.ok(disabled ? m.users_user_disabled({ name }) : m.users_user_enabled({ name })))
		);
	}
	function purgeUser(id: string, name: string) {
		if (!confirm(m.users_confirm_purge_user({ name }))) return;
		guard(
			actions.purgeUser(id).then(() => {
				toasts.ok(m.users_user_deleted());
				if (selectedId === id) selectedId = '';
			})
		);
	}

	const all = $derived(users.data ?? []);
	const active = $derived(all.filter((u) => !u.revoked_at));
	const revoked = $derived(all.filter((u) => !!u.revoked_at));
	const selected = $derived(all.find((u) => u.id === selectedId) ?? null);
</script>

<div class="bar row">
	<Heading level={1}>{m.users_title()}</Heading>
	<div class="spacer"></div>
	{#if me.data?.role === 'admin'}
		<Button control variant="primary" onclick={() => (createOpen = true)}>{m.users_new_user()}</Button>
	{/if}
</div>

<!-- Who am I: role + identity + a non-secret preview of the stored
     bearer, so "user token required" errors stop being a mystery. -->
{#if me.data}
	{@const meData = me.data}
	<div class="whoami">
		<Card padding="sm">
			<div class="row whoami-row">
				<Text tone="faint">{m.users_signed_in_as()}</Text>
				<Badge tone={meData.role === 'admin' ? 'warn' : meData.role === 'user' ? 'ok' : 'neutral'}>{meData.role}</Badge>
				{#if meData.user_name}<Text weight="semibold">{meData.user_name}</Text>{/if}
				<Text variant="code" tone="faint" size="xs">{meData.token_preview}</Text>
				{#if meData.role === 'admin'}
					<Text tone="faint" size="xs">{m.users_admin_token_note()}</Text>
				{/if}
				<div class="spacer"></div>
				<Button onclick={() => void auth.logout()}>{m.users_log_out()}</Button>
			</div>
		</Card>
	</div>
{/if}

{#if me.data && me.data.role !== 'admin'}
	<!-- Self-service: a non-admin user manages its own keys here (mint
	     a dispatch-only key for automation, etc.) without an admin token. The ceiling is
	     read-only for them; only an admin can grant new capabilities. -->
	{#if me.data.user_id}
		<div class="detail-card">
			<UserScopes userId={me.data.user_id} isAdmin={false} isSelf={true} onsecret={showSecret} />
		</div>
	{/if}
{:else if users.isLoading}
	<div class="empty"><span class="spin"></span></div>
{:else}
	<div class="picker">
		<Field label={m.users_select_user()}>
			<Select bind:value={selectedId}>
				<option value="">{m.users_select_user_placeholder()}</option>
				<optgroup label={m.users_group_active()}>
					{#each active as u (u.id)}<option value={u.id}>{u.name}</option>{/each}
				</optgroup>
				{#if revoked.length}
					<optgroup label={m.users_group_revoked()}>
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
						<Badge tone="danger">{m.users_badge_revoked()}</Badge>
					{:else if u.disabled_at}
						<Badge tone="warn">{m.users_badge_disabled()}</Badge>
					{:else}
						<Badge tone="ok">{m.users_badge_active()}</Badge>
					{/if}
					{#if !u.revoked_at}
						<IconButton
							inline
							icon="edit"
							size={14}
							title={m.users_rename_user()}
							label={m.users_rename_user()}
							onclick={() => (renameUser = u)}
						/>
					{/if}
				</header>

				<dl class="props">
					<div class="prop">
						<dt><Text size="sm" tone="faint">{m.users_prop_created()}</Text></dt>
						<dd><Timestamp value={u.created_at} mode="date" size="sm" tone="inherit" /></dd>
					</div>
					<div class="prop">
						<dt><Text size="sm" tone="faint">{m.users_prop_active()}</Text></dt>
						<dd>
							<Switch
								checked={!u.disabled_at}
								label={m.users_prop_active()}
								title={u.disabled_at ? m.users_enable_user() : m.users_disable_user()}
								disabled={!!u.revoked_at}
								onclick={() => toggleDisabled(u.id, u.name, !u.disabled_at)}
							/>
						</dd>
					</div>
				</dl>
					<Text size="xs" tone="faint"
						>{m.users_dispatch_note_before()}<Text variant="code">dispatch</Text>{m.users_dispatch_note_after()}</Text
					>

				<footer class="acts">
					{#if u.revoked_at}
						<Button variant="danger" onclick={() => purgeUser(u.id, u.name)}
							>{m.users_delete_permanently()}</Button
						>
					{:else}
						<Button variant="danger" onclick={() => revoke(u.id, u.name)}>{m.users_revoke()}</Button>
					{/if}
				</footer>
			</Card>
		</div>

		<div class="detail-card">
			<UserScopes
				userId={u.id}
				isAdmin={me.data?.role === 'admin'}
				isSelf={me.data?.user_id === u.id}
				onsecret={showSecret}
			/>
		</div>

		<UserExpand user={selected} onsecret={showSecret} />
	{/if}
{/if}

{#if createOpen}
	<EditEntityModal
		title={m.users_new_user_title()}
		fieldLabel={m.users_field_user_name()}
		placeholder={m.users_user_name_placeholder()}
		saveLabel={m.users_create_user()}
		onsave={(name) => createUser(name)}
		onclose={() => (createOpen = false)}
	/>
{/if}

{#if renameUser}
	{@const u = renameUser}
	<EditEntityModal
		title={m.users_rename_user()}
		fieldLabel={m.users_field_user_name()}
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
		margin-bottom: var(--sp-4);
	}
	.whoami-row {
		gap: var(--sp-2);
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
