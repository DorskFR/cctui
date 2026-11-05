<script lang="ts">
	import type { OAuthAccount } from '$lib/queries';
	import { useResourceShares } from '$lib/queries';
	import ResourceShares from '$lib/components/molecules/ResourceShares.svelte';
	import { Button, Text, Timestamp } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { fullTime } from './redirect.logic';

	let {
		account: a,
		canShare,
		enabled
	}: { account: OAuthAccount; canShare: boolean; enabled: boolean } = $props();

	const shares = useResourceShares(
		() => 'account',
		() => a.id,
		() => canShare && enabled
	);
	const sharedWith = $derived((shares.data ?? []).map((s) => s.user_name).join(', '));
	let sharingOpen = $state(false);
</script>

<footer class="foot">
	{#if canShare}
		<Text as="span" size="xs" tone="muted">
			{#if sharedWith}
				{m.accounts_shared_with()} <Text as="span" size="xs" weight="semibold" tone="default">{sharedWith}</Text>
			{:else}
				{m.accounts_shared_none()}
			{/if}
		</Text>
		<Button variant="link" size="sm" style="font-size: var(--fs-xs)" onclick={() => (sharingOpen = !sharingOpen)}>
			{m.accounts_manage_sharing()}
		</Button>
	{/if}
	<span class="spacer"></span>
	<Text as="span" size="xs" tone="faint">
		{m.accounts_created()}
		<span class="num" title={fullTime(a.created_at)}
			><Timestamp value={a.created_at} mode="short-iso" details={false} tone="inherit" /></span
		>
	</Text>
</footer>
{#if canShare && sharingOpen}
	<div class="sharing">
		<ResourceShares resourceType="account" id={a.id} noun={m.accounts_share_noun()} {enabled} />
	</div>
{/if}

<style>
	/* Bands one shade off the card body instead of rules between them. */
	.foot {
		display: flex;
		align-items: center;
		gap: var(--sp-4);
		padding: var(--sp-2) var(--sp-4);
		background: var(--bg);
	}
	.sharing {
		padding: var(--sp-3) var(--sp-4);
		background: var(--bg);
	}
	.spacer {
		flex: 1;
	}
</style>
