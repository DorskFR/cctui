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
		type OAuthAccount,
		type AccountProvider
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { ghreviewUrl } from '$lib/config';
	import { providerLabel } from '$lib/providers';
	import AccountCard from '$lib/components/organisms/AccountCard.svelte';
	import GithubConnectors from '$lib/components/organisms/GithubConnectors.svelte';
	import DispatchersPanel from '$lib/components/organisms/DispatchersPanel.svelte';
	import AccountsBoard from '$lib/components/organisms/accounts/AccountsBoard.svelte';
	import AccountEditorModal from '$lib/components/organisms/accounts/AccountEditorModal.svelte';
	import { availableKinds } from '$lib/components/organisms/accounts/account-editor.logic';
	import { Button, Tabs, Text, type TabItem } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import { m } from '$lib/paraglide/messages';

	// The Connectors tab only appears when the ghreview backend is deployed.
	const reviewConfigured = $derived(ghreviewUrl() !== null);
	let tab = $state('ai');

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
	const tabs = $derived<TabItem[]>([
		{
			id: 'ai',
			label: `${m.accounts_tab_ai()} ${rows.length} · ${m.accounts_pools_count({ n: poolList.length })}`
		},
		...(reviewConfigured ? [{ id: 'connectors', label: m.accounts_tab_connectors() }] : []),
		{ id: 'dispatchers', label: m.accounts_tab_dispatchers() }
	]);

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

	function removeAccount(a: OAuthAccount) {
		if (!confirm(m.accounts_confirm_delete_account({ name: a.name }))) return;
		guard(actions.remove(a.id).then(() => toasts.ok(m.accounts_deleted())));
	}

	function removeProvider(a: OAuthAccount, p: AccountProvider) {
		if (
			!confirm(
				m.accounts_confirm_remove_provider({ provider: providerLabel(p.provider), name: a.name })
			)
		)
			return;
		guard(actions.removeProvider(a.id, p.id).then(() => toasts.ok(m.accounts_provider_removed())));
	}

	const isManaged = (a: OAuthAccount) => a.providers.length > 0 && a.providers.every((p) => p.managed);
</script>

<div class="page">
	<PageHead title={m.accounts_title()}>
		<Button onclick={() => (drafting = true)} disabled={drafting}>{m.accounts_add_pool()}</Button>
		<Button variant="primary" data-journey="new-account" onclick={() => editor?.openCreate()}>
			{m.accounts_new_account()}
		</Button>
	</PageHead>

	<Tabs {tabs} bind:value={tab} label={m.accounts_sections_label()}>
		{#snippet panel(id)}
			{#if id === 'ai'}
				<AccountsBoard
					accounts={rows}
					pools={poolList}
					loading={accounts.isLoading}
					owners={isAdmin ? activeUsers : []}
					bind:drafting
				>
					{#snippet card(a, pool, onmovepool)}
						<AccountCard
							account={a}
							{pool}
							pools={poolList}
							enabled={tab === 'ai'}
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
							onedit={() => goto(`/accounts/${a.id}`)}
							onremove={() => removeAccount(a)}
							onaddprovider={() => editor?.openAddProvider(a)}
							oneditprovider={(p) => goto(`/accounts/${a.id}?provider=${p.id}`)}
							onreauthprovider={(p) => editor?.reauth(a, p)}
							onremoveprovider={(p) => removeProvider(a, p)}
						/>
					{/snippet}
				</AccountsBoard>
			{:else if id === 'connectors'}
				<GithubConnectors />
			{:else if id === 'dispatchers'}
				<DispatchersPanel heading={false} />
			{/if}
		{/snippet}
	</Tabs>
</div>

<AccountEditorModal bind:this={editor} {rows} {isAdmin} {activeUsers} />

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
</style>
