<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import {
		useAccounts,
		useAccountActions,
		useMe,
		useUsers,
		type AccountProvider
	} from '$lib/queries';
	import { toasts } from '$lib/toast.svelte';
	import { providerLabel } from '$lib/providers';
	import { m } from '$lib/paraglide/messages';
	import { Breadcrumb, Text } from '@dorsk/tsumikit';
	import PageHead from '$lib/components/molecules/PageHead.svelte';
	import AccountEditorModal from '$lib/components/organisms/accounts/AccountEditorModal.svelte';
	import ProviderDrawer from '$lib/components/organisms/accounts/provider-drawer/ProviderDrawer.svelte';
	import { availableKinds } from '$lib/components/organisms/accounts/account-editor.logic';
	import { isPageId } from '$lib/components/organisms/accounts/provider-drawer/pages.logic';
	import IdentitySection from '$lib/components/organisms/accounts/account-page/IdentitySection.svelte';
	import EnvSection from '$lib/components/organisms/accounts/account-page/EnvSection.svelte';
	import ProvidersSection from '$lib/components/organisms/accounts/account-page/ProvidersSection.svelte';
	import DangerSection from '$lib/components/organisms/accounts/account-page/DangerSection.svelte';

	const accounts = useAccounts();
	const actions = useAccountActions();
	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const activeUsers = $derived((users.data ?? []).filter((u) => !u.revoked_at));

	const id = $derived(page.params.id ?? '');
	const rows = $derived(accounts.data ?? []);
	const account = $derived(rows.find((a) => a.id === id));
	const owner = $derived(activeUsers.find((u) => u.id === account?.user_id)?.name ?? null);
	const managed = $derived(
		!!account && account.providers.length > 0 && account.providers.every((p) => p.managed)
	);

	const guard = (p: Promise<unknown>) => p.catch((e: Error) => toasts.error(e.message));

	// `?provider=<id>&page=<section>` deep-links straight into a drawer section
	// (the soft-limit banner and usage chips link here).
	const drawerProvider = $derived(
		account?.providers.find((p) => p.id === page.url.searchParams.get('provider'))
	);
	const drawerPage = $derived.by(() => {
		const raw = page.url.searchParams.get('page');
		return raw && isPageId(raw) ? raw : undefined;
	});

	let editor = $state<AccountEditorModal>();

	function openProvider(p: AccountProvider, section?: string) {
		const url = new URL(page.url);
		url.searchParams.set('provider', p.id);
		if (section) url.searchParams.set('page', section);
		else url.searchParams.delete('page');
		goto(`${url.pathname}${url.search}`, { noScroll: true, keepFocus: true });
	}

	function closeDrawer() {
		goto(page.url.pathname, { noScroll: true, keepFocus: true });
	}

	function removeProvider(p: AccountProvider) {
		if (!account) return;
		if (
			!confirm(
				m.accounts_confirm_remove_provider({
					provider: providerLabel(p.provider),
					name: account.name
				})
			)
		)
			return;
		guard(
			actions.removeProvider(account.id, p.id).then(() => toasts.ok(m.accounts_provider_removed()))
		);
	}

	function removeAccount() {
		if (!account) return;
		if (!confirm(m.accounts_confirm_delete_account({ name: account.name }))) return;
		guard(
			actions.remove(account.id).then(() => {
				toasts.ok(m.accounts_deleted());
				goto('/accounts');
			})
		);
	}
</script>

<div class="page">
	<Breadcrumb
		items={[
			{ label: m.accounts_title(), href: '/accounts' },
			{ label: account?.name ?? m.common_loading() }
		]}
	/>

	{#if account}
		<PageHead title={account.name} />

		<div class="sections">
			<IdentitySection {account} {owner} />
			<ProvidersSection
				{account}
				{managed}
				canAddProvider={!managed && availableKinds(account).length > 0}
				onadd={() => editor?.openAddProvider(account)}
				onedit={(p) => openProvider(p)}
				onreauth={(p) => editor?.reauth(account, p)}
				onremove={removeProvider}
			/>
			<EnvSection {account} />
			<DangerSection {account} disabled={managed} ondelete={removeAccount} />
		</div>
	{:else if accounts.isLoading}
		<Text tone="faint">{m.common_loading()}</Text>
	{:else}
		<Text tone="faint">{m.account_not_found()}</Text>
	{/if}
</div>

<AccountEditorModal bind:this={editor} {rows} {isAdmin} {activeUsers} />

{#if account && drawerProvider}
	{#key drawerProvider.id}
		<ProviderDrawer
			{account}
			provider={drawerProvider}
			accounts={rows}
			initialPage={drawerPage}
			onclose={closeDrawer}
		/>
	{/key}
{/if}

<style>
	.page {
		display: flex;
		flex-direction: column;
		gap: var(--sp-3);
	}
	.sections {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
</style>
