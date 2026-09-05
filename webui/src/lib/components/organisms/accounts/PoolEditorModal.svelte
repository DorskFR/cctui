<script lang="ts">
	import { untrack } from 'svelte';
	import { errMessage } from '$lib/api';
	import { useAccountPoolActions, type OAuthAccount } from '$lib/queries';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import { toasts } from '$lib/toast.svelte';
	import { Button, Cluster, Field, IconButton, Input, Modal, Select, Switch, Text } from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	// `pool` null creates a fresh one. Member order IS the ladder under the
	// `ordered` strategy, so it is edited explicitly rather than inferred.
	let {
		pool,
		accounts,
		owners = [],
		onclose
	}: {
		pool: AccountPoolView | null;
		accounts: OAuthAccount[];
		/** Admin only: the users a new pool may belong to. */
		owners?: { id: string; name: string }[];
		onclose: () => void;
	} = $props();

	const actions = useAccountPoolActions();
	const initial = untrack(() => ({ pool, owners }));
	let ownerId = $state(initial.pool?.user_id ?? initial.owners[0]?.id ?? '');
	let name = $state(initial.pool?.name ?? '');
	let strategy = $state<'headroom' | 'ordered'>(
		initial.pool?.strategy === 'ordered' ? 'ordered' : 'headroom'
	);
	let failover = $state(initial.pool?.failover ?? false);
	let memberIds = $state<string[]>(
		[...(initial.pool?.members ?? [])]
			.sort((x, y) => x.position - y.position)
			.map((mem) => mem.account_id)
	);
	let saving = $state(false);

	const byId = $derived(new Map(accounts.map((a) => [a.id, a])));
	const addable = $derived(
		accounts.filter(
			(a) =>
				!memberIds.includes(a.id) &&
				(a.user_id === ownerId || a.pool_eligible) &&
				(owners.length === 0 || !ownerId || a.user_id === ownerId)
		)
	);

	function addMember(id: string) {
		if (id && !memberIds.includes(id)) memberIds = [...memberIds, id];
	}
	function removeMember(id: string) {
		memberIds = memberIds.filter((m2) => m2 !== id);
	}
	function move(id: string, delta: number) {
		const i = memberIds.indexOf(id);
		const j = i + delta;
		if (i < 0 || j < 0 || j >= memberIds.length) return;
		const next = [...memberIds];
		[next[i], next[j]] = [next[j], next[i]];
		memberIds = next;
	}

	async function save() {
		const trimmed = name.trim();
		if (!trimmed || saving) return;
		saving = true;
		try {
			if (pool) {
				await actions.update(pool.id, { name: trimmed, strategy, failover, accounts: memberIds });
				toasts.ok(m.pools_updated());
			} else {
				if (owners.length > 0 && !ownerId) {
					toasts.error(m.accounts_err_pick_owner());
					return;
				}
				await actions.create({
					name: trimmed,
					strategy,
					failover,
					accounts: memberIds,
					user_id: owners.length > 0 ? ownerId : null
				});
				toasts.ok(m.pools_created());
			}
			onclose();
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			saving = false;
		}
	}

	async function remove() {
		if (!pool || !confirm(m.pools_delete_confirm())) return;
		try {
			await actions.remove(pool.id);
			toasts.ok(m.pools_deleted());
			onclose();
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}
</script>

<Modal title={pool ? pool.name : m.pools_new()} {onclose} size="md">
	{#snippet body()}
		<div class="editor">
			{#if owners.length > 0 && !pool}
				<Field label={m.accounts_field_owner()}>
					<Select bind:value={ownerId}>
						{#each owners as u (u.id)}
							<option value={u.id}>{u.name}</option>
						{/each}
					</Select>
					<Text tone="faint" size="xs">{m.pools_owner_hint()}</Text>
				</Field>
			{/if}

			<Field label={m.pools_name()}>
				<Input bind:value={name} placeholder={m.pools_name_placeholder()} />
			</Field>

			<Field label={m.pools_strategy()}>
				<Select bind:value={strategy}>
					<option value="headroom">{m.pools_strategy_headroom()}</option>
					<option value="ordered">{m.pools_strategy_ordered()}</option>
				</Select>
				<Text tone="faint" size="xs">
					{strategy === 'ordered' ? m.pools_strategy_ordered_hint() : m.pools_strategy_headroom_hint()}
				</Text>
			</Field>

			<Field label={m.pools_failover()}>
				<Switch checked={failover} label={m.pools_failover()} onclick={() => (failover = !failover)} />
				<Text tone="faint" size="xs">{m.pools_failover_hint()}</Text>
			</Field>

			<Field label={m.pools_members()}>
				{#if memberIds.length === 0}
					<Text tone="muted" size="sm">{m.pools_members_empty()}</Text>
				{:else}
					<ol class="member-editor">
						{#each memberIds as id, i (id)}
							<li>
								<Text size="sm">{byId.get(id)?.name ?? id}</Text>
								<span class="grow"></span>
								{#if strategy === 'ordered'}
									<IconButton
										icon="chevron-up"
										label={m.pools_move_up()}
										disabled={i === 0}
										onclick={() => move(id, -1)}
									/>
									<IconButton
										icon="chevron-down"
										label={m.pools_move_down()}
										disabled={i === memberIds.length - 1}
										onclick={() => move(id, 1)}
									/>
								{/if}
								<IconButton icon="x" label={m.pools_remove_member()} onclick={() => removeMember(id)} />
							</li>
						{/each}
					</ol>
				{/if}
				{#if addable.length > 0}
					<Select
						value=""
						onchange={(e: Event) => {
							const el = e.currentTarget as HTMLSelectElement;
							addMember(el.value);
							el.value = '';
						}}
					>
						<option value="">{m.pools_add_member()}</option>
						{#each addable as a (a.id)}
							<option value={a.id}>{a.name}</option>
						{/each}
					</Select>
				{/if}
			</Field>
		</div>
	{/snippet}
	{#snippet footer()}
		<Cluster justify="space-between" align="center" gap="var(--sp-2)">
			{#if pool}
				<Button control variant="danger" onclick={remove}>{m.pools_delete()}</Button>
			{:else}
				<span></span>
			{/if}
			<Button control variant="primary" disabled={!name.trim() || saving} onclick={save}>
				{m.pools_save()}
			</Button>
		</Cluster>
	{/snippet}
</Modal>

<style>
	.editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.member-editor {
		display: flex;
		flex-direction: column;
		gap: var(--sp-1);
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.member-editor li {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.grow {
		flex: 1;
	}
</style>
