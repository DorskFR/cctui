<script lang="ts">
	import { useAccountShares, useAccountActions } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { Button, Input, Text, Timestamp } from '@dorsk/tsumikit';

	// "Shared with" section on an account card (CCT-510): lists the live grants,
	// a user-picker (login or UUID) to grant `use`, and a per-row revoke button.
	// Only rendered by the page for the account owner/admin — the list endpoint is
	// owner-scoped server-side, so `enabled` gates the fetch to avoid a 404 churn.
	let {
		id,
		enabled = true
	}: {
		id: string;
		enabled?: boolean;
	} = $props();

	const shares = useAccountShares(
		() => id,
		() => enabled
	);
	const actions = useAccountActions();

	let grantee = $state('');
	let busy = $state(false);

	async function grant() {
		const user = grantee.trim();
		if (!user) {
			toasts.err('Enter a user login or id');
			return;
		}
		busy = true;
		try {
			await actions.grantShare(id, { user });
			grantee = '';
			toasts.ok('Shared');
		} catch (e) {
			toasts.err((e as Error).message);
		} finally {
			busy = false;
		}
	}

	async function revoke(userId: string, name: string) {
		if (!confirm(`Revoke ${name}'s access to this account?`)) return;
		try {
			await actions.revokeShare(id, userId);
			toasts.ok('Revoked');
		} catch (e) {
			toasts.err((e as Error).message);
		}
	}

	const rows = $derived($shares.data ?? []);
</script>

<div class="shares">
	<Text as="div" tone="muted" size="xs" class="shares-head">Shared with</Text>
	{#if rows.length === 0}
		<Text as="div" tone="faint" size="xs">Not shared with anyone.</Text>
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
					<Button variant="danger" onclick={() => revoke(s.user_id, s.user_name)}>Revoke</Button>
				</li>
			{/each}
		</ul>
	{/if}
	<div class="share-add">
		<Input bind:value={grantee} placeholder="user login or id" />
		<Button disabled={busy} onclick={grant}>{busy ? 'Sharing…' : 'Share'}</Button>
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
