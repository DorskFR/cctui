<script lang="ts">
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { AccountProvider, OAuthAccount } from '$lib/queries';
	import ProviderColumn from '$lib/components/organisms/accounts/ProviderColumn.svelte';
	import AccountCardHead from '$lib/components/organisms/accounts/card/AccountCardHead.svelte';
	import AccountCardFoot from '$lib/components/organisms/accounts/card/AccountCardFoot.svelte';
	import AccountRedirectForm from '$lib/components/organisms/accounts/card/AccountRedirectForm.svelte';
	import { openRedirectTargets } from '$lib/components/organisms/accounts/card/redirect.logic';
	import { accountHandleDrag } from '$lib/components/organisms/accounts/handleDrag.svelte';
	import { Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	let {
		account,
		pool = null,
		pools = [],
		enabled = true,
		managed = false,
		canAddProvider = false,
		canShare = false,
		showOwner = false,
		compact = false,
		redirects = [],
		redirectTargets = [],
		onedit,
		onremove,
		onmovepool,
		onaddprovider,
		oneditprovider,
		onreauthprovider,
		onsetredirect,
		onclearredirect
	}: {
		account: OAuthAccount;
		/** The pool this account sits in, if any. */
		pool?: AccountPoolView | null;
		/** Every pool the account may be moved to; the menu is the no-drag path. */
		pools?: AccountPoolView[];
		/** Gates the lazy usage + shares fetches (the tab must be visible). */
		enabled?: boolean;
		/** Every provider is server-managed → the account is read-only. */
		managed?: boolean;
		canAddProvider?: boolean;
		canShare?: boolean;
		showOwner?: boolean;
		/** Read-only gauge view (the stats dock): no drag, menu, sharing or per-provider management. Redirect chips still show, without their clear affordance. */
		compact?: boolean;
		redirects?: { id: string; family: string; targetName: string; until: string | null }[];
		redirectTargets?: { id: string; name: string; families: string[] }[];
		onedit?: () => void;
		onremove?: () => void;
		onmovepool?: (pool: AccountPoolView | null) => void;
		onaddprovider?: () => void;
		oneditprovider?: (p: AccountProvider) => void;
		onreauthprovider?: (p: AccountProvider) => void;
		onsetredirect?: (targetId: string, untilHours: number | null, families: string[]) => void;
		onclearredirect?: (ruleId: string) => void;
	} = $props();

	const a = $derived(account);
	const ownFamilies = $derived(a.providers.map((p) => p.family));
	const openTargets = $derived(openRedirectTargets(redirectTargets, ownFamilies, redirects));
	let redirectOpen = $state(false);

	const drag = accountHandleDrag({
		accountId: () => a.id,
		pool: () => pool,
		pools: () => pools,
		onmovepool: () => onmovepool
	});
</script>

<article
	class="acct"
	class:compact
	class:lifted={drag.touchDragging}
	id={a.id}
	data-journey="account"
	data-journey-key={a.name}
>
	<AccountCardHead
		account={a}
		{pool}
		{pools}
		{drag}
		{enabled}
		{managed}
		{canAddProvider}
		{showOwner}
		{compact}
		{redirects}
		canRedirect={!!onsetredirect && openTargets.length > 0}
		{onedit}
		{onremove}
		{onmovepool}
		{onaddprovider}
		{onclearredirect}
		ontoggleredirect={() => (redirectOpen = !redirectOpen)}
	/>

	{#if redirectOpen}
		<AccountRedirectForm
			targets={openTargets}
			{ownFamilies}
			{redirects}
			onsubmit={(targetId, untilHours, families) => {
				onsetredirect?.(targetId, untilHours, families);
				redirectOpen = false;
			}}
			oncancel={() => (redirectOpen = false)}
		/>
	{/if}

	<div class="columns">
		{#each a.providers as p (p.id)}
			<ProviderColumn
				provider={p}
				usageEnabled={enabled}
				canManage={!p.managed && !compact}
				dense={compact}
				onedit={() => oneditprovider?.(p)}
				onreauth={() => onreauthprovider?.(p)}
			/>
		{:else}
			<div class="none"><Text tone="faint" size="sm">{m.accounts_no_credentials()}</Text></div>
		{/each}
	</div>

	{#if !compact}
		<AccountCardFoot account={a} {canShare} {enabled} />
	{/if}
</article>

<style>
	/* Depth from the elevated shade; the head and foot are bands, not rules. */
	.acct {
		container: acct-row / inline-size;
		display: flex;
		flex-direction: column;
		min-width: 0;
		border-radius: var(--r-lg);
		background: var(--bg-elevated);
		overflow: hidden;
	}
	.acct.lifted {
		box-shadow: var(--shadow-lg);
		outline: 2px solid var(--accent);
		opacity: 0.85;
	}
	/* Whitespace, not rules, between provider columns. */
	.columns {
		display: grid;
		grid-template-columns: repeat(3, minmax(0, 1fr));
		column-gap: var(--sp-6);
	}
	.none {
		padding: var(--sp-3) var(--sp-4);
	}
	@container acct-row (max-width: 66rem) {
		.columns {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
	@container acct-row (max-width: 34rem) {
		.columns {
			grid-template-columns: minmax(0, 1fr);
		}
	}
</style>
