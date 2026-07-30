<script lang="ts">
	// Per-user scope (ceiling) editing + per-key (grant) management.
	// Admin can edit any user's ceiling and manage any user's keys; a user can
	// view their own ceiling (read-only) and manage their own keys. Key scopes
	// are editable in place — the secret is never re-minted to re-scope.
	import { errMessage } from '$lib/api';
	import { useUserAcls, useUserKeys, useUserActions } from '$lib/queries';
	import type { ApiKeyRow } from '@bindings/ApiKeyRow';
	import { toasts } from '$lib/toast.svelte';
	import { Badge, Button, Card, Heading, IconButton, Input, Switch, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		userId,
		isAdmin,
		isSelf,
		onsecret
	}: {
		userId: string;
		isAdmin: boolean;
		isSelf: boolean;
		onsecret: (title: string, value: string) => void;
	} = $props();

	const ALL_SCOPES = ['read', 'dispatch', 'enroll', 'admin'] as const;
	type ScopeName = (typeof ALL_SCOPES)[number];

	const acls = useUserAcls(() => userId);
	const keys = useUserKeys(() => userId);
	const actions = useUserActions();
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.err(e.message));

	const ceiling = $derived(new Set(($acls.data?.scopes ?? []) as ScopeName[]));
	// Only an admin may grant a user new capabilities; a user sees its own
	// ceiling read-only.
	const canEditCeiling = $derived(isAdmin);
	// Managing keys: self always, or admin cross-user.
	const canManageKeys = $derived(isAdmin || isSelf);

	function toggleCeiling(scope: ScopeName, on: boolean) {
		const next = new Set(ceiling);
		if (on) next.add(scope);
		else next.delete(scope);
		guard(
			actions
				.setUserScopes(userId, [...next])
				.then(() => toasts.ok(on ? m.users_scope_granted({ scope }) : m.users_scope_revoked({ scope })))
		);
	}

	// New-key form state.
	let newLabel = $state('');
	let newScopes = $state<Set<ScopeName>>(new Set(['read']));
	function toggleNewScope(scope: ScopeName, on: boolean) {
		const next = new Set(newScopes);
		if (on) next.add(scope);
		else next.delete(scope);
		newScopes = next;
	}
	async function mint() {
		try {
			const r = await actions.mintKey(userId, newLabel.trim() || null, [...newScopes]);
			onsecret(m.users_key_secret_title({ label: newLabel.trim() || m.users_unlabeled_plain() }), r.key);
			newLabel = '';
			newScopes = new Set(['read']);
		} catch (e) {
			toasts.err(errMessage(e));
		}
	}

	function toggleKeyScope(key: ApiKeyRow, scope: ScopeName, on: boolean) {
		const next = new Set(key.scopes as ScopeName[]);
		if (on) next.add(scope);
		else next.delete(scope);
		guard(actions.setKeyScopes(userId, key.id, [...next]).then(() => toasts.ok(m.users_key_scopes_updated())));
	}
	function revokeKey(key: ApiKeyRow) {
		if (!confirm(m.users_confirm_revoke_key({ key: key.label ?? key.key_preview ?? key.id })))
			return;
		guard(actions.revokeKey(userId, key.id).then(() => toasts.ok(m.users_key_revoked())));
	}

	// A scope can be granted to a key only if it's within the owner's ceiling.
	const liveKeys = $derived(($keys.data ?? []).filter((k) => !k.revoked_at));
	const revokedKeys = $derived(($keys.data ?? []).filter((k) => !!k.revoked_at));
</script>

<Card>
	<Heading level={3}>{m.users_scopes_ceiling_title()}</Heading>
	<Text as="p" size="sm" tone="faint">
		{m.users_scopes_ceiling_help()}
	</Text>
	<div class="scopes">
		{#each ALL_SCOPES as s (s)}
			<Switch
				checked={ceiling.has(s)}
				label={s}
				disabled={!canEditCeiling}
				title={canEditCeiling ? m.users_scope_toggle({ scope: s }) : m.users_scope_admin_required()}
				onclick={() => toggleCeiling(s, !ceiling.has(s))}
			/>
		{/each}
	</div>
</Card>

{#if canManageKeys}
	<Card>
		<Heading level={3}>{m.users_api_keys_title()}</Heading>
		<Text as="p" size="sm" tone="faint">
			{m.users_api_keys_help()}
		</Text>

		<div class="mint">
			<Input mono placeholder={m.users_label_optional_placeholder()} bind:value={newLabel} />
			<div class="scopes">
				{#each ALL_SCOPES as s (s)}
					<Switch
						checked={newScopes.has(s)}
						label={s}
						disabled={!ceiling.has(s)}
						title={ceiling.has(s) ? m.users_scope_grant({ scope: s }) : m.users_scope_not_in_ceiling()}
						onclick={() => toggleNewScope(s, !newScopes.has(s))}
					/>
				{/each}
			</div>
			<Button variant="primary" onclick={mint} disabled={newScopes.size === 0}>{m.users_mint_key()}</Button>
		</div>

		{#if liveKeys.length}
			<ul class="keys">
				{#each liveKeys as k (k.id)}
					<li class="key">
						<div class="key-head">
							<Text weight="semibold">{k.label ?? m.users_unlabeled()}</Text>
							<Badge tone="neutral">{k.kind}</Badge>
							<Text variant="code" tone="faint" size="xs">{k.key_preview ?? '••••'}</Text>
							<div class="spacer"></div>
							<Timestamp value={k.created_at} mode="date" size="xs" tone="faint" />
							<IconButton inline icon="trash" size={14} title={m.users_revoke_key()} label={m.users_revoke_key()} onclick={() => revokeKey(k)} />
						</div>
						<div class="scopes">
							{#each ALL_SCOPES as s (s)}
								<Switch
									checked={(k.scopes as string[]).includes(s)}
									label={s}
									disabled={!ceiling.has(s)}
									title={ceiling.has(s) ? m.users_scope_toggle_key({ scope: s }) : m.users_scope_not_in_ceiling()}
									onclick={() => toggleKeyScope(k, s, !(k.scopes as string[]).includes(s))}
								/>
							{/each}
						</div>
					</li>
				{/each}
			</ul>
		{:else}
			<Text as="p" tone="faint" size="sm">{m.users_no_active_keys()}</Text>
		{/if}

		{#if revokedKeys.length}
			<details class="revoked">
				<summary><Text size="sm" tone="faint">{m.users_revoked_keys({ count: revokedKeys.length })}</Text></summary>
				<ul class="keys">
					{#each revokedKeys as k (k.id)}
						<li class="key dim">
							<Text size="sm">{k.label ?? m.users_unlabeled()}</Text>
							<Text variant="code" tone="faint" size="xs">{k.key_preview ?? '••••'}</Text>
							<Badge tone="danger">{m.users_badge_revoked()}</Badge>
						</li>
					{/each}
				</ul>
			</details>
		{/if}
	</Card>
{/if}

<style>
	.scopes {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-3);
		margin: var(--sp-2) 0;
	}
	.mint {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3) 0;
		border-bottom: 1px solid var(--border);
		margin-bottom: var(--sp-3);
	}
	.keys {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.key {
		border: 1px solid var(--border);
		border-radius: var(--radius-2);
		padding: var(--sp-2) var(--sp-3);
	}
	.key.dim {
		opacity: 0.6;
		display: flex;
		gap: var(--sp-2);
		align-items: center;
	}
	.key-head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
	}
	.spacer {
		flex: 1;
	}
	.revoked {
		margin-top: var(--sp-3);
	}
</style>
