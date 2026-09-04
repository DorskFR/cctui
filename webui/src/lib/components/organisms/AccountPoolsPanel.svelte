<!--
  Account pools: the named sets of accounts a session is allowed to run on.

  This screen exists because account movement used to have no visible object
  behind it. "Auto" ranked every account the user could reach, and the gateway
  moved live sessions the same way, so personal work could land on a work
  credential with nothing on screen saying that was allowed. A pool is that
  missing statement, made editable: name it, put accounts in it, and decide
  whether a running session may be moved between them.

  Deliberately NOT the same object as a redirect rule (edited per account, on
  the AI tab): a redirect is dated and one-off — "A is spent until tonight" —
  while a pool is standing policy. Same screen, two panels, never one knob.
-->
<script lang="ts">
	import { errMessage } from '$lib/api';
	import {
		useAccountPools,
		useAccountPoolActions,
		useAccounts,
		useMe,
		useUsers,
		type OAuthAccount,
	} from '$lib/queries';
	import type { AccountPoolView } from '@bindings/AccountPoolView';
	import { toasts } from '$lib/toast.svelte';
	import {
		Badge,
		Button,
		Cluster,
		Field,
		Heading,
		IconButton,
		Input,
		Modal,
		Select,
		Switch,
		Text,
	} from '@dorsk/tsumikit';
	import { m } from '$lib/paraglide/messages';

	const pools = useAccountPools();
	const accounts = useAccounts();
	const actions = useAccountPoolActions();
	// A pool belongs to a user, and the admin token is not one: it has no user
	// identity, so it must name the owner on create or the server refuses with
	// "user_id required when using the admin token". Same picker, same default
	// as the accounts screen's owner field.
	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const activeUsers = $derived((users.data ?? []).filter((u) => !u.revoked_at));
	let ownerId = $state('');
	$effect(() => {
		if (isAdmin && !ownerId && activeUsers.length) ownerId = activeUsers[0].id;
	});

	// Editor state. `editing` holds the pool being edited, null while creating
	// a fresh one, undefined when the editor is closed.
	let editing = $state<AccountPoolView | null | undefined>(undefined);
	let name = $state('');
	let strategy = $state<'headroom' | 'ordered'>('headroom');
	let failover = $state(false);
	// Member account ids, in election order — the order IS the ladder under the
	// `ordered` strategy, so it is edited explicitly rather than inferred.
	let memberIds = $state<string[]>([]);
	let saving = $state(false);

	const accountList = $derived(accounts.data ?? []);
	const byId = $derived(new Map(accountList.map((a: OAuthAccount) => [a.id, a])));
	// Accounts not yet in the pool being edited, and not withheld from pools by
	// their owner — the latter would only be refused by the server.
	// An admin sees every user's accounts, so the list is also narrowed to the
	// pool's owner: sharing grants are not visible from here, and offering an
	// account the owner cannot reach only buys a 400 from `check_members`.
	const addable = $derived(
		accountList.filter(
			(a: OAuthAccount) =>
				!memberIds.includes(a.id) &&
				a.pool_eligible &&
				(!isAdmin || !ownerId || a.user_id === ownerId)
		)
	);
	const ownerName = (id: string) =>
		activeUsers.find((u) => u.id === id)?.name ?? accountList.find((a) => a.user_id === id)?.user_name ?? '';

	function openCreate() {
		// Back to the default owner: openEdit() leaves ownerId on the pool it
		// last showed.
		if (isAdmin) ownerId = activeUsers[0]?.id ?? '';
		name = '';
		strategy = 'headroom';
		failover = false;
		memberIds = [];
		editing = null;
	}

	function openEdit(p: AccountPoolView) {
		// PATCH is owner-scoped server-side; the owner of an existing pool is
		// never re-parented from here, only shown.
		ownerId = p.user_id;
		name = p.name;
		strategy = p.strategy === 'ordered' ? 'ordered' : 'headroom';
		failover = p.failover;
		memberIds = p.members.map((mem) => mem.account_id);
		editing = p;
	}

	function close() {
		editing = undefined;
	}

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
			if (editing) {
				await actions.update(editing.id, {
					name: trimmed,
					strategy,
					failover,
					accounts: memberIds,
				});
				toasts.ok(m.pools_updated());
			} else {
				if (isAdmin && !ownerId) {
					toasts.err(m.accounts_err_pick_owner());
					return;
				}
				await actions.create({
					name: trimmed,
					strategy,
					failover,
					accounts: memberIds,
					user_id: isAdmin ? ownerId : null,
				});
				toasts.ok(m.pools_created());
			}
			close();
		} catch (e) {
			toasts.err(errMessage(e));
		} finally {
			saving = false;
		}
	}

	async function remove(p: AccountPoolView) {
		if (!confirm(m.pools_delete_confirm())) return;
		try {
			await actions.remove(p.id);
			toasts.ok(m.pools_deleted());
			close();
		} catch (e) {
			toasts.err(errMessage(e));
		}
	}

	const strategyLabel = (s: string) =>
		s === 'ordered' ? m.pools_strategy_ordered() : m.pools_strategy_headroom();
</script>

<div class="pools-pane">
	<Cluster justify="space-between" align="center" gap="var(--sp-3)">
		<Text as="p" tone="muted" size="sm">{m.pools_intro()}</Text>
		<Button control variant="primary" onclick={openCreate}>{m.pools_new()}</Button>
	</Cluster>

	{#if pools.isLoading}
		<div class="empty"><span class="spin"></span></div>
	{:else if (pools.data ?? []).length === 0}
		<div class="empty"><Text tone="muted">{m.pools_empty()}</Text></div>
	{:else}
		<div class="pool-list">
			{#each pools.data ?? [] as p (p.id)}
				<div class="pool-row">
					<div class="pool-head">
						<Heading level={3}>{p.name}</Heading>
						{#if isAdmin && ownerName(p.user_id)}
							<Badge tone="neutral">{ownerName(p.user_id)}</Badge>
						{/if}
						<Badge tone="neutral">{strategyLabel(p.strategy)}</Badge>
						{#if p.failover}
							<Badge tone="info">{m.pools_failover()}</Badge>
						{/if}
						<div class="grow"></div>
						<Button control onclick={() => openEdit(p)}>{m.common_edit()}</Button>
					</div>
					{#if p.members.length === 0}
						<Text tone="muted" size="sm">{m.pools_members_empty()}</Text>
					{:else}
						<div class="members">
							{#each p.members as mem (mem.account_id)}
								<span class="member" class:withheld={!mem.owned && !mem.pool_eligible}>
									<Text size="sm">{mem.name}</Text>
									{#if !mem.owned}
										<Text size="xs" tone="faint">
											{mem.pool_eligible ? m.pools_member_shared() : m.pools_member_withheld()}
										</Text>
									{/if}
								</span>
							{/each}
						</div>
					{/if}
				</div>
			{/each}
		</div>
	{/if}
</div>

{#if editing !== undefined}
	<Modal title={editing ? editing.name : m.pools_new()} onclose={close} size="md">
		{#snippet body()}
			<div class="editor">
				{#if isAdmin && !editing}
					<Field label={m.accounts_field_owner()}>
						<Select bind:value={ownerId}>
							{#each activeUsers as u (u.id)}
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
						{strategy === 'ordered'
							? m.pools_strategy_ordered_hint()
							: m.pools_strategy_headroom_hint()}
					</Text>
				</Field>

				<Field label={m.pools_failover()}>
					<Switch
						checked={failover}
						label={m.pools_failover()}
						onclick={() => (failover = !failover)}
					/>
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
									<div class="grow"></div>
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
									<IconButton
										icon="x"
										label={m.pools_remove_member()}
										onclick={() => removeMember(id)}
									/>
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
				{#if editing}
					<Button control variant="danger" onclick={() => remove(editing!)}>
						{m.pools_delete()}
					</Button>
				{:else}
					<span></span>
				{/if}
				<Button control variant="primary" disabled={!name.trim() || saving} onclick={save}>
					{m.pools_save()}
				</Button>
			</Cluster>
		{/snippet}
	</Modal>
{/if}

<style>
	.pools-pane {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.empty {
		display: flex;
		justify-content: center;
		padding: var(--sp-6) 0;
	}
	.pool-list {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.pool-row {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--border);
		border-radius: var(--radius-md);
		background: var(--surface);
	}
	.pool-head {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
	}
	.grow {
		flex: 1;
	}
	.members {
		display: flex;
		flex-wrap: wrap;
		gap: var(--sp-2);
	}
	.member {
		display: inline-flex;
		align-items: baseline;
		gap: var(--sp-1);
		padding: var(--sp-1) var(--sp-2);
		border: 1px solid var(--border);
		border-radius: var(--radius-sm);
	}
	/* A member its owner has withdrawn stays listed — the row is why it stopped
	   counting, not clutter — but reads as inert. */
	.member.withheld {
		opacity: 0.55;
		border-style: dashed;
	}
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
</style>
