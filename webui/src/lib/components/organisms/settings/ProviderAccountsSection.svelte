<script lang="ts">
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import AccountsPanel from '#lib/components/organisms/accounts/AccountsPanel.svelte';
	import { m } from '#lib/paraglide/messages.js';

	let { active = false }: { active?: boolean } = $props();
</script>

<SettingSection id="provider-accounts" icon="⚷" title={m.settings_sec_provider_accounts()}>
	<SettingGroup>
		<SettingRow
			label={m.settings_provider_accounts_label()}
			help={m.settings_provider_accounts_help()}
			selfLabelled
			wide
		/>
	</SettingGroup>
	{#if active}
		<AccountsPanel />
	{/if}
</SettingSection>
