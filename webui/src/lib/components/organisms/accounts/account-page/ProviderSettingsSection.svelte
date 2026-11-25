<script lang="ts">
	import type { AccountProvider, OAuthAccount } from '#lib/queries/index.ts';
	import { providerLabel } from '#lib/providers.ts';
	import { Button, Card, ConfirmModal, Tabs, Text } from '@dorsk/tsumikit';
	import { m } from '#lib/paraglide/messages.js';
	import ProviderSettings from '../provider-drawer/ProviderSettings.svelte';
	import type { PageId } from '../provider-drawer/pages.logic';

	let {
		account,
		providers,
		selected,
		accounts = [],
		initialPage,
		canRemove = false,
		onselect,
		onmoved,
		onremove
	}: {
		account: OAuthAccount;
		providers: AccountProvider[];
		selected: AccountProvider;
		accounts?: OAuthAccount[];
		initialPage?: PageId;
		canRemove?: boolean;
		onselect: (p: AccountProvider) => void;
		onmoved: () => void;
		onremove?: (p: AccountProvider) => Promise<unknown>;
	} = $props();

	let confirmingRemove = $state(false);
	const selectedLabel = $derived(providerLabel(selected.provider));

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
	{#if canRemove && onremove}
		<div class="danger" data-journey="provider-danger">
			<Text as="p" tone="muted" size="sm" measure="60ch">
				{m.providers_remove_help({ provider: selectedLabel })}
			</Text>
			<span class="spacer"></span>
			<Button size="sm" variant="danger" onclick={() => (confirmingRemove = true)}>
				{m.providers_remove({ provider: selectedLabel })}
			</Button>
		</div>
	{/if}
</Card>

<ConfirmModal
	bind:open={confirmingRemove}
	tone="danger"
	title={m.providers_remove_title({ provider: selectedLabel, name: account.name })}
	message={m.providers_remove_confirm({ provider: selectedLabel, name: account.name })}
	confirmLabel={m.providers_remove({ provider: selectedLabel })}
	cancelLabel={m.common_cancel()}
	onconfirm={async () => {
		await onremove?.(selected);
	}}
/>

<style>
	.danger {
		display: flex;
		align-items: center;
		flex-wrap: wrap;
		gap: var(--sp-3);
		margin-top: var(--sp-4);
		padding: var(--sp-3) var(--sp-4);
		border-radius: var(--r-md);
		background: color-mix(in srgb, var(--danger) 8%, transparent);
	}
	.spacer {
		flex: 1;
	}
</style>
