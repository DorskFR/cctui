<script lang="ts">
	// CCT-410: per-user scope (ceiling) editing + per-key (grant) management.
	// Admin can edit any user's ceiling and manage any user's keys; a user can
	// view their own ceiling (read-only) and manage their own keys. Key scopes
	// are editable in place — the secret is never re-minted to re-scope.
	import { useUserAcls, useUserKeys, useUserActions } from '$lib/queries';
	import type { ApiKeyRow } from '@bindings/ApiKeyRow';
	import { toasts } from '$lib/toast.svelte';
	import { Badge, Button, Card, Heading, IconButton, Input, Switch, Text, Timestamp } from '@dorsk/tsumikit';

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
				.then(() => toasts.ok(`Scope ${scope} ${on ? 'granted' : 'revoked'}`))
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
			onsecret(`Key — ${newLabel.trim() || 'unlabeled'}`, r.key);
			newLabel = '';
			newScopes = new Set(['read']);
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	function toggleKeyScope(key: ApiKeyRow, scope: ScopeName, on: boolean) {
		const next = new Set(key.scopes as ScopeName[]);
		if (on) next.add(scope);
		else next.delete(scope);
		guard(actions.setKeyScopes(userId, key.id, [...next]).then(() => toasts.ok('Key scopes updated')));
	}
	function revokeKey(key: ApiKeyRow) {
		if (!confirm(`Revoke key ${key.label ?? key.key_preview ?? key.id}? It stops working immediately.`))
			return;
		guard(actions.revokeKey(userId, key.id).then(() => toasts.ok('Key revoked')));
	}

	// A scope can be granted to a key only if it's within the owner's ceiling.
	const liveKeys = $derived(($keys.data ?? []).filter((k) => !k.revoked_at));
	const revokedKeys = $derived(($keys.data ?? []).filter((k) => !!k.revoked_at));
</script>

<Card>
	<Heading level={3}>Scopes (ceiling)</Heading>
	<Text as="p" size="sm" tone="faint">
		The capabilities this user may delegate to its keys. A key's effective authority is its grant
		intersected with this ceiling — removing a scope here immediately limits every key.
	</Text>
	<div class="scopes">
		{#each ALL_SCOPES as s (s)}
			<Switch
				checked={ceiling.has(s)}
				label={s}
				disabled={!canEditCeiling}
				title={canEditCeiling ? `Toggle ${s}` : 'Admin scope required to edit the ceiling'}
				onclick={() => toggleCeiling(s, !ceiling.has(s))}
			/>
		{/each}
	</div>
</Card>

{#if canManageKeys}
	<Card>
		<Heading level={3}>API keys</Heading>
		<Text as="p" size="sm" tone="faint">
			Mint scoped tokens (e.g. a dispatch-only key for automation). Scopes are editable in place — the
			secret is never re-minted to re-scope. New grants are clamped to the ceiling above.
		</Text>

		<div class="mint">
			<Input mono placeholder="label (optional)" bind:value={newLabel} />
			<div class="scopes">
				{#each ALL_SCOPES as s (s)}
					<Switch
						checked={newScopes.has(s)}
						label={s}
						disabled={!ceiling.has(s)}
						title={ceiling.has(s) ? `Grant ${s}` : 'Not in the ceiling'}
						onclick={() => toggleNewScope(s, !newScopes.has(s))}
					/>
				{/each}
			</div>
			<Button variant="primary" onclick={mint} disabled={newScopes.size === 0}>+ Mint key</Button>
		</div>

		{#if liveKeys.length}
			<ul class="keys">
				{#each liveKeys as k (k.id)}
					<li class="key">
						<div class="key-head">
							<Text weight="semibold">{k.label ?? '(unlabeled)'}</Text>
							<Badge tone="neutral">{k.kind}</Badge>
							<Text variant="code" tone="faint" size="xs">{k.key_preview ?? '••••'}</Text>
							<div class="spacer"></div>
							<Timestamp value={k.created_at} mode="date" size="xs" tone="faint" />
							<IconButton inline icon="trash" size={14} title="Revoke key" label="Revoke key" onclick={() => revokeKey(k)} />
						</div>
						<div class="scopes">
							{#each ALL_SCOPES as s (s)}
								<Switch
									checked={(k.scopes as string[]).includes(s)}
									label={s}
									disabled={!ceiling.has(s)}
									title={ceiling.has(s) ? `Toggle ${s} on this key` : 'Not in the ceiling'}
									onclick={() => toggleKeyScope(k, s, !(k.scopes as string[]).includes(s))}
								/>
							{/each}
						</div>
					</li>
				{/each}
			</ul>
		{:else}
			<Text as="p" tone="faint" size="sm">No active keys.</Text>
		{/if}

		{#if revokedKeys.length}
			<details class="revoked">
				<summary><Text size="sm" tone="faint">Revoked keys ({revokedKeys.length})</Text></summary>
				<ul class="keys">
					{#each revokedKeys as k (k.id)}
						<li class="key dim">
							<Text size="sm">{k.label ?? '(unlabeled)'}</Text>
							<Text variant="code" tone="faint" size="xs">{k.key_preview ?? '••••'}</Text>
							<Badge tone="danger">revoked</Badge>
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
