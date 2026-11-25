<script lang="ts">
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import { useAccountActions, useAccounts, useMe } from '#lib/queries/index.ts';
	import { errMessage } from '#lib/api.ts';
	import { toasts } from '#lib/toast.svelte.ts';
	import { ownedAccounts } from './accountSettings.logic';
	import { m } from '#lib/paraglide/messages.js';

	const me = useMe();
	const accounts = useAccounts();
	const actions = useAccountActions();
	const owned = $derived(ownedAccounts(accounts.data, me.data?.user_id));
	let busy = $state<string | null>(null);

	async function setEligible(id: string, on: boolean) {
		busy = id;
		try {
			await actions.update(id, { pool_eligible: on });
		} catch (e) {
			toasts.error(errMessage(e));
		} finally {
			busy = null;
		}
	}
</script>

<SettingSection id="pools" icon="⧉" title={m.settings_sec_pools()} description={m.settings_pools_desc()}>
	<SettingGroup title={m.settings_pool_eligible_title()}>
		{#each owned as a (a.id)}
			<SettingRow label={a.emoji ? `${a.emoji} ${a.name}` : a.name} help={m.settings_pool_eligible_help()}>
				<Switch
					bind:checked={() => a.pool_eligible, (v) => void setEligible(a.id, v)}
					disabled={busy === a.id}
					label={m.settings_pool_eligible_named({ name: a.name })}
					data-testid="pool-eligible"
					data-account={a.id}
				/>
			</SettingRow>
		{:else}
			<SettingRow label={m.settings_pool_eligible_none()} selfLabelled wide />
		{/each}
	</SettingGroup>
</SettingSection>
