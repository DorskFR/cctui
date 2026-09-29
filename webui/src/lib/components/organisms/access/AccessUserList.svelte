<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import { Avatar, Input, Button, Disclosure, Dot, Spinner, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import EnrollMachineCard from '$lib/components/organisms/EnrollMachineCard.svelte';
	import { hashHue } from '$lib/format';
	import { m } from '$lib/paraglide/messages';
	import { filterByName, splitRevoked } from './access.logic';

	let {
		users,
		loading = false,
		selectedId,
		canCreate = false,
		meta,
		online,
		onselect,
		oncreate
	}: {
		users: UserRow[];
		loading?: boolean;
		selectedId: string;
		canCreate?: boolean;
		meta: (u: UserRow) => string;
		online: (u: UserRow) => boolean;
		onselect: (id: string) => void;
		oncreate: () => void;
	} = $props();

	let query = $state('');
	const matched = $derived(filterByName(users, query));
	const groups = $derived(splitRevoked(matched));
</script>

{#snippet entry(u: UserRow, revoked: boolean)}
	<div class="row" class:on={u.id === selectedId} class:dim={revoked}>
		<Button
			variant="ghost"
			block
			style="justify-content:flex-start; gap:var(--sp-2); padding:10px var(--sp-3); border-radius:0"
			data-journey="user"
			data-journey-key={u.name}
			aria-current={u.id === selectedId ? 'true' : undefined}
			onclick={() => onselect(u.id)}
		>
			<Avatar name={u.name} hue={hashHue(u.name)} size={26} decorative />
			<span class="id">
				<span class="nm">{u.name}</span>
				<span class="mt">{meta(u)}</span>
			</span>
			{#if !revoked}
				<Dot status={online(u) ? 'active' : 'dead'} />
			{/if}
		</Button>
	</div>
{/snippet}

<div class="master">
	<PageHead title={m.access_title()}>
		{#if canCreate}
			<Button variant="primary" onclick={oncreate}>{m.users_new_user()}</Button>
		{/if}
	</PageHead>

	<Input
		icon="search"
		type="search"
		aria-label={m.access_filter_placeholder()}
		placeholder={m.access_filter_placeholder()}
		bind:value={query}
	/>

	<div class="list">
		{#if loading}
			<div class="msg"><Spinner /></div>
		{:else if matched.length === 0}
			<div class="msg"><Text size="sm" tone="faint">{m.access_no_users()}</Text></div>
		{:else}
			{#each groups.active as u (u.id)}{@render entry(u, false)}{/each}
			{#if groups.revoked.length}
				<div class="revoked">
					<Disclosure>
						{#snippet header()}
							<Text size="xs" tone="faint"
								>{m.access_revoked_group({ count: groups.revoked.length })}</Text
							>
						{/snippet}
						{#each groups.revoked as u (u.id)}{@render entry(u, true)}{/each}
					</Disclosure>
				</div>
			{/if}
		{/if}
	</div>

	<EnrollMachineCard dashed />
</div>

<style>
	.master {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
		padding-block: 0 var(--sp-4);
		padding-inline-end: var(--sp-4);
	}
	/* Stacked under the breakpoint there is no divider to keep off. */
	@media (max-width: 47.999rem) {
		.master {
			padding-inline-end: 0;
		}
	}
	.list {
		border: 1px solid var(--border);
		border-radius: var(--r-md);
		background: var(--bg-elevated);
		overflow: hidden;
	}
	.msg {
		display: grid;
		place-items: center;
		padding: var(--sp-4);
	}
	.row {
		border-left: 2px solid transparent;
		border-bottom: 1px solid var(--border);
	}
	.row:last-child {
		border-bottom: 0;
	}
	.row.on {
		background: var(--bg-elevated-2);
		border-left-color: var(--accent);
	}
	.row.dim {
		opacity: 0.55;
	}
	.id {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		text-align: start;
	}
	.nm {
		font-size: var(--fs-sm);
		font-weight: var(--fw-medium);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.mt {
		font-size: var(--fs-xs);
		color: var(--text-faint);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.revoked {
		border-top: 1px solid var(--border);
	}
</style>
