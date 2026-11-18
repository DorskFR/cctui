<script lang="ts">
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import ProfileList from '../spawn/ProfileList.svelte';
	import type { ProfileSpecForm } from '../spawn/profiles';
	import {
		useAccountPools,
		useAccounts,
		useAllAccountsUsage,
		useProfileActions,
		useProfiles
	} from '#lib/queries/index.ts';
	import { errMessage } from '#lib/api.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const profiles = useProfiles();
	const actions = useProfileActions();
	const accounts = useAccounts();
	const pools = useAccountPools(() => hasFeature('account_sharing'));
	const usage = useAllAccountsUsage();

	let selectedId = $state<string | null>(null);
	let oneOff = $state<ProfileSpecForm | null>(null);
	let busy = $state(false);

	async function guarded(run: () => Promise<void>): Promise<boolean> {
		busy = true;
		try {
			await run();
			return true;
		} catch (e) {
			toasts.error(m.spawn_profile_toast_failed({ error: errMessage(e) }));
			return false;
		} finally {
			busy = false;
		}
	}

	const create = (name: string, spec: ProfileSpecForm) =>
		guarded(async () => {
			const p = await actions.create({ name, ...spec });
			selectedId = p.id;
			oneOff = null;
		});

	const save = (id: string, name: string, spec: ProfileSpecForm) =>
		void guarded(async () => {
			await actions.update(id, { name, spec });
			toasts.ok(m.spawn_profile_toast_saved());
		});

	const remove = (id: string) =>
		void guarded(async () => {
			await actions.remove(id);
			if (selectedId === id) selectedId = null;
		});
</script>

<SettingGroup title={m.settings_profiles_title()}>
	<SettingRow label={m.settings_profiles_label()} help={m.settings_profiles_help()} wide selfLabelled>
		<ProfileList
			profiles={profiles.data ?? []}
			bind:selectedId
			bind:oneOff
			accounts={accounts.data ?? []}
			pools={pools.data ?? []}
			usage={usage.data ?? []}
			machineId=""
			{busy}
			oncreate={create}
			onsave={save}
			ondelete={remove}
		/>
	</SettingRow>
</SettingGroup>
