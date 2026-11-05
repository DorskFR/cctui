<script lang="ts">
	// Users, their keys, machines and tokens. An admin browses every identity
	// through the master/detail list; everyone else manages exactly one — their
	// own — so the list is dropped rather than rendered with a single row.
	import type { UserRow } from '@bindings/UserRow';
	import { MasterDetail, Text } from '@dorsk/tsumikit';
	import AccessDetail from './AccessDetail.svelte';
	import AccessUserList from './AccessUserList.svelte';
	import EnrollMachineCard from '$lib/components/organisms/EnrollMachineCard.svelte';
	import EditEntityModal from '$lib/components/molecules/EditEntityModal.svelte';
	import SecretReveal from '$lib/components/molecules/SecretReveal.svelte';
	import {
		useAccounts,
		useAllMachines,
		useHiddenOwnerActions,
		useHiddenOwners,
		useMe,
		useUserActions,
		useUsers
	} from '$lib/queries';
	import { canHideUser, isHiddenOwner } from '$lib/hiddenOwners';
	import { userStateWords } from './access.logic';
	import { toasts } from '$lib/toast.svelte';
	import { errMessage } from '$lib/api';
	import { m } from '$lib/paraglide/messages';

	const me = useMe();
	const isAdmin = $derived(me.data?.role === 'admin');
	const users = useUsers(() => isAdmin);
	const machines = useAllMachines(() => isAdmin);
	const accounts = useAccounts();
	const hiddenOwners = useHiddenOwners(() => isAdmin);
	const userHidden = (u: UserRow) => isHiddenOwner(hiddenOwners.data?.owners, 'user', u.id);
	const hiddenActions = useHiddenOwnerActions();
	function setVisible(u: UserRow, visible: boolean) {
		const work = visible ? hiddenActions.unhide('user', u.id) : hiddenActions.hide('user', u.id, u.name);
		work.catch((e: unknown) => toasts.error(errMessage(e)));
	}
	const actions = useUserActions();

	const self = $derived<UserRow | null>(
		me.data?.user_id
			? {
					id: me.data.user_id,
					name: me.data.user_name ?? me.data.user_id,
					created_at: '',
					revoked_at: null,
					disabled_at: null,
					can_dispatch: true,
					last_seen_at: null
				}
			: null
	);
	const all = $derived(users.data ?? []);

	let picked = $state<string | null>(null);
	const selectedId = $derived(picked ?? '');
	const selected = $derived(all.find((u) => u.id === selectedId) ?? null);

	const accountsOf = (userId: string) => (accounts.data ?? []).filter((a) => a.user_id === userId);
	const machinesOf = (userId: string) => (machines.data ?? []).filter((mc) => mc.user_id === userId);
	const userMeta = (u: UserRow) => {
		if (u.revoked_at) return m.access_revoked_meta();
		const counts = m.access_user_meta({ machines: machinesOf(u.id).length, accounts: accountsOf(u.id).length });
		const online = machinesOf(u.id).filter((mc) => mc.liveness === 'online').length;
		return `${counts} · ${userStateWords(online, machinesOf(u.id).length)}`;
	};

	let secret = $state<{ title: string; value: string } | null>(null);
	let createOpen = $state(false);

	async function createUser(name: string | null) {
		if (!name) return;
		try {
			const r = await actions.create(name);
			picked = r.id;
			secret = { title: m.users_key_secret_title({ label: r.name }), value: r.key };
		} catch (e) {
			toasts.error(errMessage(e));
		}
	}
</script>

{#if isAdmin}
	<MasterDetail
		listWidth="272px"
		breakpoint="48rem"
		selected={!!selected}
		onback={() => (picked = '')}
		backLabel={m.settings_nav_users()}
		listLabel={m.settings_nav_users()}
	>
		{#snippet list()}
			<AccessUserList
				users={all}
				loading={users.isLoading}
				{selectedId}
				canCreate
				meta={userMeta}
				hidden={userHidden}
				hideable={(u) => canHideUser(me.data?.user_id, u.id)}
				onvisible={setVisible}
				onselect={(id) => (picked = id)}
				oncreate={() => (createOpen = true)}
			/>
		{/snippet}

		{#snippet empty()}
			<div class="placeholder"><Text tone="faint">{m.access_pick_user()}</Text></div>
		{/snippet}

		{#snippet detail()}
			{#if selected}
				<AccessDetail
					user={selected}
					isAdmin
					isSelf={me.data?.user_id === selected.id}
					accounts={accountsOf(selected.id)}
					accountsLoading={accounts.isLoading}
					onsecret={(title, value) => (secret = { title, value })}
					ongone={() => (picked = '')}
				/>
			{/if}
		{/snippet}
	</MasterDetail>
{:else if self}
	<div class="solo">
		<EnrollMachineCard />
		<AccessDetail
			user={self}
			isAdmin={false}
			isSelf
			accounts={accountsOf(self.id)}
			accountsLoading={accounts.isLoading}
			onsecret={(title, value) => (secret = { title, value })}
			ongone={() => {}}
		/>
	</div>
{/if}

{#if createOpen}
	<EditEntityModal
		title={m.users_new_user_title()}
		fieldLabel={m.users_field_user_name()}
		placeholder={m.users_user_name_placeholder()}
		saveLabel={m.users_create_user()}
		onsave={(name) => createUser(name)}
		onclose={() => (createOpen = false)}
	/>
{/if}

{#if secret}
	<SecretReveal title={secret.title} secret={secret.value} onclose={() => (secret = null)} />
{/if}

<style>
	.solo {
		display: flex;
		flex-direction: column;
		gap: var(--sp-4);
	}
	.placeholder {
		display: grid;
		place-items: center;
		padding: var(--sp-6);
	}
</style>
