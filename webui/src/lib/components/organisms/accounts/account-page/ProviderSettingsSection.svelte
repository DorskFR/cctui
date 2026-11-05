<script lang="ts">
	import type { AccountProvider, OAuthAccount } from '$lib/queries';
	import { providerLabel } from '$lib/providers';
	import { Button, Card, Tabs, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import ProviderSettings from '../provider-drawer/ProviderSettings.svelte';
	import type { PageId } from '../provider-drawer/pages.logic';

	let {
		account,
		providers,
		selected,
		accounts = [],
		initialPage,
		onselect,
		onmoved,
		onremove
	}: {
		account: OAuthAccount;
		providers: AccountProvider[];
		selected: AccountProvider;
		accounts?: OAuthAccount[];
		initialPage?: PageId;
		onselect: (p: AccountProvider) => void;
		onmoved: () => void;
		/** Absent: no danger zone. */
		onremove?: (p: AccountProvider) => void;
	} = $props();

	const tabs = $derived(providers.map((p) => ({ id: p.id, label: providerLabel(p.provider) })));

	function pick(id: string | undefined) {
		const hit = providers.find((p) => p.id === id);
		if (hit && hit.id !== selected.id) onselect(hit);
	}
</script>

<Card
	id="provider-settings"
	title={m.account_section_provider_settings()}
	subtitle={providers.length > 1
		? m.account_section_provider_settings_help()
		: providerLabel(selected.provider)}
>
	{#if providers.length > 1}
		<Tabs
			{tabs}
			bind:value={() => selected.id, pick}
			label={m.account_section_provider_settings()}
		/>
	{/if}
	{#key selected.id}
		<ProviderSettings
			{account}
			provider={selected}
			{accounts}
			{initialPage}
			{onmoved}
		/>
	{/key}
	{#if onremove}
		<div class="danger" data-journey="provider-danger">
			<div class="danger-text">
				<Text as="h3" size="sm" weight="semibold" tone="danger">{m.account_section_danger()}</Text>
				<Text as="p" tone="muted" size="sm" measure="60ch">
					{m.acct_remove_provider_help({ provider: providerLabel(selected.provider), name: account.name })}
				</Text>
			</div>
			<Button size="sm" variant="danger" onclick={() => onremove?.(selected)}>
				{m.acct_remove_provider_confirm()}
			</Button>
		</div>
	{/if}
</Card>

<style>
	.danger {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		justify-content: space-between;
		gap: var(--sp-3);
		margin-top: var(--sp-4);
		padding: var(--sp-3);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--danger) 10%, var(--bg-elevated));
	}
	.danger-text {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
	}
</style>
