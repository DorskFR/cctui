<script lang="ts">
	import type { AccountProvider, OAuthAccount } from '#lib/queries/index.ts';
	import ProviderColumn from '../ProviderColumn.svelte';
	import { canEditProvider } from '../account-editor.logic';
	import { Button, Card, Text } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';

	let {
		account,
		managed = false,
		owned = false,
		canAddProvider = false,
		onadd,
		onedit,
		onreauth,
		onremove
	}: {
		account: OAuthAccount;
		managed?: boolean;
		owned?: boolean;
		canAddProvider?: boolean;
		onadd?: () => void;
		onedit?: (p: AccountProvider) => void;
		onreauth?: (p: AccountProvider) => void;
		onremove?: (p: AccountProvider) => void;
	} = $props();
</script>

<Card title={m.account_section_providers()} subtitle={m.account_section_providers_help()}>
	{#snippet actions()}
		{#if canAddProvider}
			<Button size="sm" onclick={() => onadd?.()}>{m.accounts_add_provider()}</Button>
		{/if}
	{/snippet}

	<div class="grid">
		{#each account.providers as p (p.id)}
			<ProviderColumn
				provider={p}
				canManage={canEditProvider(p, owned) && !managed}
				canRemove={canEditProvider(p, owned) && !managed}
				onedit={() => onedit?.(p)}
				onreauth={() => onreauth?.(p)}
				onremove={() => onremove?.(p)}
			/>
		{:else}
			<Text tone="faint" size="sm">{m.accounts_no_credentials()}</Text>
		{/each}
	</div>
</Card>

<style>
	.grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(16rem, 1fr));
		gap: var(--sp-3);
	}
</style>
