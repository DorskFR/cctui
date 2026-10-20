<script lang="ts">
	import type { AccountProvider, OAuthAccount } from '$lib/queries';
	import { providerLabel } from '$lib/providers';
	import { Card, Tabs } from '@dorsk/tsumikit';
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
		onmoved
	}: {
		account: OAuthAccount;
		providers: AccountProvider[];
		selected: AccountProvider;
		accounts?: OAuthAccount[];
		initialPage?: PageId;
		onselect: (p: AccountProvider) => void;
		onmoved: () => void;
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
</Card>
