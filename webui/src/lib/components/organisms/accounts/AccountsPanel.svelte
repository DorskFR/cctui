<script lang="ts">
	import { goto } from '$app/navigation';
	import {
		useAccounts,
		useAccountActions,
		useAccountPools,
		useMe,
		useRedirectChips,
		useRedirectActions,
		useUsers,
		type OAuthAccount
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import AccountCard from '$lib/components/organisms/AccountCard.svelte';
	import AccountsBoard from './AccountsBoard.svelte';
	import AccountEditorModal from './AccountEditorModal.svelte';
	import { availableKinds } from './account-editor.logic';
	import { accountHref } from './account-page/account-nav.logic';
	import { confirmDeleteAccount } from './account-page/confirms';
	import { Button } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { m } from '$lib/paraglide/messages';

	let { heading = true }: { heading?: boolean } = $props();

	const accounts = useAccounts();
	const pools = useAccountPools();
	const actions = useAccountActions();
	const redirectChips = useRedirectChips();
	const redirectActions = useRedirectActions();
	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const activeUsers = $derived((users.data ?? []).filter((u) => !u.revoked_at));
	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.error(e.message));

	const rows = $derived([...(accounts.data ?? [])]);
	const poolList = $derived(pools.data ?? []);

	let drafting = $state(false);
	let editor = $state<AccountEditorModal>();

	const redirectsFor = (accountId: string) => redirectChips.chipsFor(accountId);
	const redirectTargetsFor = (acct: OAuthAccount) =>
		rows
			.filter((t) => t.id !== acct.id)
			.map((t) => ({ id: t.id, name: t.name, families: t.providers.map((p) => p.family) }))
			.filter((t) => t.families.length > 0);
	const setRedirect = (
		acct: OAuthAccount,
		targetId: string,
		untilHours: number | null,
		families: string[]
	) => {
		const until =
			untilHours === null ? null : new Date(Date.now() + untilHours * 3_600_000).toISOString();
		guard(
			Promise.all(
				families.map((family) =>
					redirectActions.put(acct.id, {
						to_account: targetId,
						family,
						until,
						user_id: isAdmin ? acct.user_id : null
					})
				)
			)
		);
	};
	const clearRedirect = (ruleId: string) => guard(redirectActions.remove(ruleId));

	async function removeAccount(a: OAuthAccount) {
		if (!(await confirmDeleteAccount(a.name))) return;
		guard(actions.remove(a.id).then(() => toasts.ok(m.accounts_deleted())));
	}

	const isManaged = (a: OAuthAccount) => a.providers.length > 0 && a.providers.every((p) => p.managed);
</script>

{#snippet headActions()}
	<Button data-journey="new-pool" onclick={() => (drafting = true)} disabled={drafting}>{m.accounts_add_pool()}</Button>
	<Button variant="primary" data-journey="new-account" onclick={() => editor?.openCreate()}>
		{m.accounts_new_account()}
	</Button>
{/snippet}

<div class="panel">
	{#if heading}
		<PageHead title={m.accounts_title()}>{@render headActions()}</PageHead>
	{:else}
		<div class="actions">{@render headActions()}</div>
	{/if}

	<AccountsBoard
		accounts={rows}
		pools={poolList}
		loading={accounts.isLoading}
		owners={isAdmin ? activeUsers : []}
		selfId={me.data?.user_id ?? null}
		bind:drafting
	>
		{#snippet card(a, pool, onmovepool)}
			<AccountCard
				account={a}
				{pool}
				pools={poolList}
				enabled
				managed={isManaged(a)}
				canAddProvider={!isManaged(a) && availableKinds(a).length > 0}
				canShare={!isManaged(a) && (isAdmin || a.user_id === me.data?.user_id)}
				showOwner={isAdmin}
				redirects={redirectsFor(a.id)}
				redirectTargets={redirectTargetsFor(a)}
				onsetredirect={(targetId, untilHours, families) =>
					setRedirect(a, targetId, untilHours, families)}
				onclearredirect={clearRedirect}
				{onmovepool}
				onedit={() => goto(accountHref(a.id))}
				onremove={() => removeAccount(a)}
				onaddprovider={() => editor?.openAddProvider(a)}
				oneditprovider={(p) => goto(accountHref(a.id, p.id))}
				onreauthprovider={(p) => editor?.reauth(a, p)}
			/>
		{/snippet}
	</AccountsBoard>
</div>

<AccountEditorModal bind:this={editor} {rows} {isAdmin} {activeUsers} />

<style>
	.panel {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--sp-2);
	}
</style>
