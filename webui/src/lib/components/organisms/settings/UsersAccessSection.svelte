<script lang="ts">
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import AccessPanel from '#lib/components/organisms/access/AccessPanel.svelte';
	import DispatchersPanel from '#lib/components/organisms/DispatchersPanel.svelte';
	import SessionVisibilityGroup from './SessionVisibilityGroup.svelte';
	import { m } from '#lib/paraglide/messages.js';

	let { isAdmin = false, active = false }: { isAdmin?: boolean; active?: boolean } = $props();
</script>

<SettingSection id="instance-users" icon="◎" title={m.settings_sec_users()}>
	<SettingGroup>
		<SettingRow label={m.settings_nav_users()} help={m.settings_users_help()} selfLabelled wide />
		<SettingRow label={m.settings_nav_dispatchers()} help={m.settings_dispatchers_help()} selfLabelled wide />
	</SettingGroup>
	<SessionVisibilityGroup {isAdmin} />
	{#if active}
		<div id="instance-users-list" data-journey="page" data-journey-key="users">
			<AccessPanel />
		</div>
		<div id="instance-dispatchers" data-journey="page" data-journey-key="dispatchers">
			<DispatchersPanel />
		</div>
	{/if}
</SettingSection>
