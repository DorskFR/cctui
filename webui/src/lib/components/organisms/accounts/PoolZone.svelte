<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import type { OAuthAccount } from '$lib/queries';
	import { Fieldset, IconButton, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';
	import { ACCOUNT_DRAG_MIME, acceptsDrop } from './pools.logic';
	import { accountDrag } from './drag.svelte';

	let {
		pool,
		accounts,
		ownerName = null,
		onedit,
		ondrop,
		children
	}: {
		pool: AccountPoolView;
		/** Every account on the page, to judge a dropped id. */
		accounts: OAuthAccount[];
		/** Shown to admins, who see everyone's pools. */
		ownerName?: string | null;
		onedit?: () => void;
		ondrop?: (accountId: string) => void;
		children?: Snippet;
	} = $props();

	const meta = $derived(
		pool.failover
			? m.pools_legend_failover({ n: pool.members.length })
			: m.pools_legend({ n: pool.members.length })
	);
	const dragged = $derived(accounts.find((a) => a.id === accountDrag.accountId)?.name ?? '');
</script>

<Fieldset
	tone="accent"
	dashed
	padding="sm"
	droppable
	mime={ACCOUNT_DRAG_MIME}
	accepts={(id) => acceptsDrop(pool, id || accountDrag.accountId, accounts)}
	ondrop={(id) => ondrop?.(id || accountDrag.accountId)}
	dropHint={m.pools_drop_hint({ name: dragged })}
	class="pool"
>
	{#snippet legend()}
		<Text as="span" size="sm" weight="semibold" tone="accent">{pool.name}</Text>
		<Text as="span" size="xs" tone="faint">{ownerName ? `${ownerName} · ${meta}` : meta}</Text>
		{#if onedit}
			<IconButton icon="edit" label={m.pools_edit()} inline size={13} onclick={onedit} />
		{/if}
	{/snippet}
	<div class="members">
		{@render children?.()}
		{#if pool.members.length === 0}
			<Text as="p" tone="faint" size="sm">{m.pools_members_empty()}</Text>
		{/if}
	</div>
</Fieldset>

<style>
	.members {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
</style>
