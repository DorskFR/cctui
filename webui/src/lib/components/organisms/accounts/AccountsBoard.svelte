<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import { errMessage } from '$lib/api';
	import { useAccountPoolActions, type OAuthAccount } from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import NewPoolZone from './NewPoolZone.svelte';
	import PoolEditorModal from './PoolEditorModal.svelte';
	import PoolZone from './PoolZone.svelte';
	import { groupAccounts, membershipAfterMove } from './pools.logic';

	let {
		accounts,
		pools,
		loading = false,
		owners = [],
		drafting = $bindable(false),
		card
	}: {
		accounts: OAuthAccount[];
		pools: AccountPoolView[];
		loading?: boolean;
		/** Admin only: users a pool may belong to; also labels each pool's owner. */
		owners?: { id: string; name: string }[];
		/** The empty "+ Add pool" zone, first so its name field is in reach on a phone. */
		drafting?: boolean;
		card: Snippet<[OAuthAccount, AccountPoolView | null, (to: AccountPoolView | null) => void]>;
	} = $props();

	const actions = useAccountPoolActions();
	const groups = $derived(groupAccounts(accounts, pools));
	const ownerName = (id: string) => owners.find((u) => u.id === id)?.name ?? null;

	let editing = $state<AccountPoolView | null | undefined>(undefined);
	let busy = $state(false);

	async function move(accountId: string, to: AccountPoolView | null) {
		const changes = membershipAfterMove(pools, accountId, to);
		if (changes.length === 0 || busy) return;
		busy = true;
		try {
			await actions.move(changes);
			toasts.ok(m.pools_updated());
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}

	async function createPool(name: string, accountId: string | null) {
		if (busy) return;
		busy = true;
		try {
			const account = accountId ? accounts.find((a) => a.id === accountId) : null;
			if (account) await actions.move(membershipAfterMove(pools, account.id, null));
			await actions.create({
				name,
				strategy: 'headroom',
				failover: false,
				accounts: account ? [account.id] : [],
				user_id: owners.length > 0 ? (account?.user_id ?? owners[0]?.id ?? null) : null
			});
			toasts.ok(m.pools_created());
			drafting = false;
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = false;
		}
	}
</script>

<div class="board">
	{#if loading}
		<div class="empty"><span class="spin"></span></div>
	{:else if accounts.length === 0 && !drafting}
		<div class="empty"><Text tone="muted">{m.accounts_empty()}</Text></div>
	{:else}
		{#if drafting}
			<NewPoolZone {accounts} {busy} oncreate={createPool} ondiscard={() => (drafting = false)} />
		{/if}
		{#each groups.pooled as g (g.pool.id)}
			<PoolZone
				pool={g.pool}
				{accounts}
				ownerName={ownerName(g.pool.user_id)}
				onedit={() => (editing = g.pool)}
				ondrop={(id) => move(id, g.pool)}
			>
				{#each g.accounts as a (a.id)}
					{@render card(a, g.pool, (to) => move(a.id, to))}
				{/each}
			</PoolZone>
		{/each}
		{#each groups.solo as a (a.id)}
			{@render card(a, null, (to) => move(a.id, to))}
		{/each}
	{/if}
</div>

{#if editing !== undefined}
	<PoolEditorModal pool={editing} {accounts} {owners} onclose={() => (editing = undefined)} />
{/if}

<style>
	.board {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.empty {
		display: flex;
		justify-content: center;
		padding: var(--sp-6) 0;
	}
</style>
