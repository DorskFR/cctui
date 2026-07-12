<script lang="ts">
	import { useResourceShares, useResourceShareActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { Button, Input, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// "Shared with" section for any shareable resource (CCT-531): lists live
	// grants, a user-picker (login or UUID) to grant `use`, and a per-row revoke.
	// Only rendered for the resource owner/admin — the list endpoint is
	// owner-scoped server-side, so `enabled` gates the fetch to avoid 404 churn.
	let {
		resourceType,
		id,
		noun = m.providers_share_noun_default(),
		enabled = true
	}: {
		resourceType: string;
		id: string;
		noun?: string;
		enabled?: boolean;
	} = $props();

	const shares = useResourceShares(
		() => resourceType,
		() => id,
		() => enabled
	);
	const actions = useResourceShareActions();

	let grantee = $state('');
	let busy = $state(false);

	async function grant() {
		const user = grantee.trim();
		if (!user) {
			toasts.err(m.providers_share_err_enter_user());
			return;
		}
		busy = true;
		try {
			await actions.grant(resourceType, id, { user });
			grantee = '';
			toasts.ok(m.providers_share_shared());
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function revoke(userId: string, name: string) {
		if (!confirm(m.providers_share_confirm_revoke({ name, noun }))) return;
		try {
			await actions.revoke(resourceType, id, userId);
			toasts.ok(m.providers_share_revoked());
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	const rows = $derived($shares.data ?? []);
</script>

<div class="shares">
	<Text as="div" tone="muted" size="xs" class="shares-head">{m.providers_share_heading()}</Text>
	{#if rows.length === 0}
		<Text as="div" tone="faint" size="xs">{m.providers_share_empty()}</Text>
	{:else}
		<ul class="share-list">
			{#each rows as s (s.user_id)}
				<li class="share-row">
					<span class="share-who">
						<Text as="span" size="sm">{s.user_name}</Text>
						<Text as="span" tone="faint" size="xs">
							{s.action} · <Timestamp value={s.granted_at} mode="relative" tone="inherit" />
						</Text>
					</span>
					<Button variant="danger" onclick={() => revoke(s.user_id, s.user_name)}>{m.providers_share_revoke()}</Button>
				</li>
			{/each}
		</ul>
	{/if}
	<div class="share-add">
		<Input bind:value={grantee} placeholder={m.providers_share_input_placeholder()} />
		<Button disabled={busy} onclick={grant}>{busy ? m.providers_share_sharing() : m.providers_share_button()}</Button>
	</div>
</div>

<style>
	.shares {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--r-sm);
		background: var(--bg-elevated-2);
	}
	.shares :global(.shares-head) {
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.share-list {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
	.share-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-2);
		min-width: 0;
	}
	.share-who {
		display: flex;
		flex-direction: column;
		min-width: 0;
		overflow-wrap: anywhere;
	}
	.share-add {
		display: grid;
		grid-template-columns: 1fr auto;
		gap: var(--sp-2);
		align-items: center;
	}
</style>
