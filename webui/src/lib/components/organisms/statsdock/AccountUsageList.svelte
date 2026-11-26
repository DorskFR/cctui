<script lang="ts">
	import { useAccounts, useRedirectChips } from '#lib/queries/index.ts';
	import AccountCard from '#lib/components/organisms/AccountCard.svelte';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';

	const accounts = useAccounts();
	const rows = $derived(accounts.data ?? []);
	const chips = useRedirectChips();
</script>

{#if accounts.isLoading}
	<Text tone="faint" size="sm">{m.common_loading()}</Text>
{:else if rows.length === 0}
	<Text tone="faint" size="sm">{m.stats_dock_no_accounts()}</Text>
{:else}
	<div class="list">
		{#each rows as a (a.id)}
			<AccountCard account={a} compact showOwner redirects={chips.chipsFor(a.id)} />
		{/each}
	</div>
{/if}

<style>
	/* The dock is itself --bg-elevated; lifting the card body one step makes
	   header and body one surface that stands off the panel. */
	.list {
		--bg-elevated: var(--bg-elevated-2);
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
	}
</style>
