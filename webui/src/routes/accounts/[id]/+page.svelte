<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import {
		useAccounts,
		useAccountActions,
		useMe,
		useUsers,
		type AccountProvider
	} from '#lib/queries/index.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { tick } from 'svelte';
	import { Breadcrumb, Button, ConfirmModal, Text } from '@dorsk/tsumikit';
	import PageHead from '#lib/components/molecules/PageHead.svelte';
	import AccountEditorModal from '#lib/components/organisms/accounts/AccountEditorModal.svelte';
	import { availableKinds, ownsAccount } from '#lib/components/organisms/accounts/account-editor.logic.ts';
	import {
		ACCOUNTS_HREF,
		accountHref,
		editableProviders,
		pageParam,
		pickProvider
	} from '#lib/components/organisms/accounts/account-page/account-nav.logic.ts';
	import IdentitySection from '#lib/components/organisms/accounts/account-page/IdentitySection.svelte';
	import EnvSection from '#lib/components/organisms/accounts/account-page/EnvSection.svelte';
	import ProvidersSection from '#lib/components/organisms/accounts/account-page/ProvidersSection.svelte';
	import ToolPolicySection from '#lib/components/organisms/accounts/account-page/ToolPolicySection.svelte';
	import ServiceTokensSection from '#lib/components/organisms/accounts/account-page/ServiceTokensSection.svelte';
	import DangerSection from '#lib/components/organisms/accounts/account-page/DangerSection.svelte';
	import ProviderSettingsSection from '#lib/components/organisms/accounts/account-page/ProviderSettingsSection.svelte';

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

	const editable = $derived(account ? editableProviders(account, managed) : []);
	const requested = $derived(page.url.searchParams.get('provider'));
	const selected = $derived(pickProvider(editable, requested));
	const landingPage = $derived(pageParam(page.url.searchParams.get('page')));

	let editor = $state<AccountEditorModal>();

	async function revealSettings() {
		await tick();
		document.getElementById('provider-settings')?.scrollIntoView({ block: 'start' });
	}

	function selectProvider(p: AccountProvider, reveal = false) {
		if (!account) return;
		goto(accountHref(account.id, p.id), { replace: true, reset: false });
		if (reveal) revealSettings();
	}

	function dropSelection() {
		if (account) goto(accountHref(account.id), { replace: true, reset: false });
	}

	let revealed = false;
	$effect(() => {
		if (revealed || !requested || !selected) return;
		revealed = true;
		revealSettings();
	});

	function removeProvider(p: AccountProvider) {
		if (!account) return Promise.resolve();
		return actions.removeProvider(account.id, p.id).then(() => {
			toasts.ok(m.accounts_provider_removed());
			dropSelection();
		});
	}

	let confirmingDelete = $state(false);
	function removeAccount() {
		if (!account) return;
		guard(
			actions.remove(account.id).then(() => {
				toasts.ok(m.accounts_deleted());
				goto(ACCOUNTS_HREF);
			})
		);
	}
</script>

<div class="page">
	<Breadcrumb
		items={[
			{ label: m.accounts_title(), href: ACCOUNTS_HREF },
			{ label: account?.name ?? m.common_loading() }
		]}
	/>

	{#if account}
		<PageHead title={account.name}>
			<Button as="a" href={ACCOUNTS_HREF}>{m.account_back()}</Button>
		</PageHead>

		<div class="sections">
			<IdentitySection {account} {owner} />
			<ProvidersSection
				{account}
				{managed}
				owned={ownsAccount(account, me.data)}
				canAddProvider={!managed && availableKinds(account).length > 0}
				onadd={() => editor?.openAddProvider(account)}
				onedit={(p) => selectProvider(p, true)}
				onreauth={(p) => editor?.reauth(account, p)}
			/>
			{#if selected}
				<ProviderSettingsSection
					{account}
					providers={editable}
					{selected}
					accounts={rows}
					initialPage={landingPage}
					canRemove={ownsAccount(account, me.data)}
					onselect={(p) => selectProvider(p)}
					onmoved={dropSelection}
					onremove={removeProvider}
				/>
			{/if}
			<EnvSection {account} />
			{#if !managed}
				<ToolPolicySection {account} />
				{#if hasFeature('service_tokens')}
					<ServiceTokensSection {account} />
				{/if}
			{/if}
			<DangerSection {account} disabled={managed} ondelete={() => (confirmingDelete = true)} />
		</div>
	{:else if accounts.isLoading}
		<Text tone="faint">{m.common_loading()}</Text>
	{:else}
		<Text tone="faint">{m.account_not_found()}</Text>
	{/if}
</div>

<AccountEditorModal bind:this={editor} {rows} {isAdmin} {activeUsers} />

{#if account}
	<ConfirmModal
		bind:open={confirmingDelete}
		tone="danger"
		title={m.accounts_delete_account()}
		message={m.accounts_confirm_delete_account({ name: account.name })}
		confirmLabel={m.accounts_delete_account()}
		cancelLabel={m.common_cancel()}
		onconfirm={removeAccount}
	/>
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
