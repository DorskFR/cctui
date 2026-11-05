<script lang="ts">
	import type { UserRow } from '@bindings/UserRow';
	import { Avatar, Badge, Input, Button, Disclosure, Spinner, Switch, Text } from '@dorsk/tsumikit';
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
		hidden = () => false,
		hideable = () => true,
		onvisible,
		onselect,
		oncreate
	}: {
		users: UserRow[];
		loading?: boolean;
		selectedId: string;
		canCreate?: boolean;
		/** The muted second line: counts and state in words. */
		meta: (u: UserRow) => string;
		hidden?: (u: UserRow) => boolean;
		hideable?: (u: UserRow) => boolean;
		/** Present for admins: the "Show sessions in my list" switch per user. */
		onvisible?: (u: UserRow, visible: boolean) => void;
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
			style="flex:1; min-width:0; justify-content:flex-start; gap:var(--sp-2); padding:10px var(--sp-3); border-radius:0"
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
			{#if hidden(u)}
				<Badge tone="muted" size="xs" style="flex:none" data-journey="hidden-chip">{m.hidden_owners_chip()}</Badge>
			{/if}
		</Button>
		{#if onvisible && !revoked && hideable(u)}
			<span class="vis">
				<Switch
					size="sm"
					bind:checked={() => !hidden(u), (v) => onvisible(u, v)}
					label={m.set_visibility_user_label({ name: u.name })}
					data-journey="user-visibility"
					data-journey-key={u.id}
				/>
			</span>
		{/if}
	</div>
{/snippet}

<div class="master">
	{#if canCreate}
		<div class="head">
			<Button variant="primary" onclick={oncreate}>{m.users_new_user()}</Button>
		</div>
	{/if}

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

	<EnrollMachineCard />
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
	.head {
		display: flex;
		justify-content: flex-end;
	}
	.row {
		display: flex;
		align-items: center;
		border-left: 2px solid transparent;
		border-bottom: 1px solid var(--border);
	}
	.vis {
		display: inline-flex;
		flex: none;
		padding-inline-end: var(--sp-3);
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
		gap: var(--sp-1);
		text-align: start;
	}
	.nm {
		font-size: var(--fs-sm);
		font-weight: var(--fw-semibold);
		line-height: var(--lh-snug, 1.3);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.mt {
		font-size: var(--fs-xs);
		line-height: var(--lh-snug, 1.3);
		color: var(--text-muted);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.revoked {
		border-top: 1px solid var(--border);
	}
</style>
