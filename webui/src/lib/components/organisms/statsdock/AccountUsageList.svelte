<script lang="ts">
	import { useAccounts } from '$lib/queries';
	import AccountCard from '$lib/components/organisms/AccountCard.svelte';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// Every account the viewer can see, as the same organism the Accounts
	// screen shows, in its read-only gauge form.
	const accounts = useAccounts();
	const rows = $derived(accounts.data ?? []);
</script>

{#if accounts.isLoading}
	<Text tone="faint" size="sm">{m.common_loading()}</Text>
{:else if rows.length === 0}
	<Text tone="faint" size="sm">{m.stats_dock_no_accounts()}</Text>
{:else}
	<div class="list">
		{#each rows as a (a.id)}
			<AccountCard account={a} compact showOwner />
		{/each}
	</div>
{/if}

<style>
	.list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
