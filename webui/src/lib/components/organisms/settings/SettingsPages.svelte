<script lang="ts">
	// Every settings page, mounted at once, one shown: the search box filters
	// across pages through the rows' own localized copy. Screens with no rows of
	// their own (the accounts board, users, dispatchers) render only on their page.
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import AppearanceSection from './AppearanceSection.svelte';
	import LayoutSection from './LayoutSection.svelte';
	import ConversationDisplaySection from './ConversationDisplaySection.svelte';
	import GuidesSection from './GuidesSection.svelte';
	import ProfileSection from './ProfileSection.svelte';
	import SecuritySection from './SecuritySection.svelte';
	import MachinesSection from './MachinesSection.svelte';
	import ProviderAccountsSection from './ProviderAccountsSection.svelte';
	import PoolsSection from './PoolsSection.svelte';
	import SharingSection from './SharingSection.svelte';
	import LaunchSection from './LaunchSection.svelte';
	import ConversationSection from './ConversationSection.svelte';
	import ContextSection from './ContextSection.svelte';
	import PrivacySection from './PrivacySection.svelte';
	import AutomationSection from './AutomationSection.svelte';
	import NotificationsSection from './NotificationsSection.svelte';
	import VoiceSection from './VoiceSection.svelte';
	import PluginsSection from './PluginsSection.svelte';
	import InstanceSection from './InstanceSection.svelte';
	import UsersAccessSection from './UsersAccessSection.svelte';
	import MachineAdaptersGroup from './MachineAdaptersGroup.svelte';
	import HarnessUpdateGroup from './HarnessUpdateGroup.svelte';
	import SpawnLimitsGroup from './SpawnLimitsGroup.svelte';
	import UploadsSection from './UploadsSection.svelte';
	import EventRetentionGroup from './EventRetentionGroup.svelte';
	import UpstreamHostsGroup from './UpstreamHostsGroup.svelte';
	import SpeechGroup from './SpeechGroup.svelte';
	import PluginsAdminGroup from './PluginsAdminGroup.svelte';
	import ServerStorageSection from './ServerStorageSection.svelte';
	import RedteamSection from './RedteamSection.svelte';
	import type { SettingsPage } from './settings.logic';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	let { current, isAdmin }: { current: SettingsPage; isAdmin: boolean } = $props();
</script>

<div class="pg" class:on={current === 'interface'} data-settings-page="interface" data-journey="page" data-journey-key="interface">
	<AppearanceSection />
	<LayoutSection />
	<ConversationDisplaySection />
	<GuidesSection />
</div>

<div class="pg" class:on={current === 'accounts'} data-settings-page="accounts" data-journey="page" data-journey-key="accounts">
	<ProfileSection {isAdmin} active={current === 'accounts'} />
	<SecuritySection />
	{#if hasFeature('machine_spawn')}
		<MachinesSection {isAdmin} />
	{/if}
	<ProviderAccountsSection active={current === 'accounts'} />
	{#if hasFeature('account_sharing')}
		<PoolsSection />
		<SharingSection active={current === 'accounts'} />
	{/if}
</div>

<div class="pg" class:on={current === 'sessions'} data-settings-page="sessions" data-journey="page" data-journey-key="sessions">
	<LaunchSection />
	<ConversationSection />
	{#if hasFeature('context')}
		<ContextSection />
	{/if}
	<PrivacySection />
	<AutomationSection />
</div>

<div class="pg" class:on={current === 'notifications'} data-settings-page="notifications" data-journey="page" data-journey-key="notifications">
	<NotificationsSection />
	{#if hasFeature('voice')}
		<VoiceSection />
	{/if}
</div>

{#if hasFeature('plugins')}
	<div class="pg" class:on={current === 'plugins'} data-settings-page="plugins" data-journey="page" data-journey-key="plugins">
		<PluginsSection />
	</div>
{/if}

{#if isAdmin}
	<div class="pg" class:on={current === 'instance'} data-settings-page="instance" data-journey="page" data-journey-key="instance">
		<InstanceSection {isAdmin} />
		<UsersAccessSection {isAdmin} active={current === 'instance'} />
		<SettingSection id="instance-machines" icon="▣" title={m.settings_sec_instance_machines()} admin>
			{#if hasFeature('machine_spawn')}
				<MachineAdaptersGroup isAdmin />
			{/if}
			{#if hasFeature('self_update')}
				<HarnessUpdateGroup />
			{/if}
		</SettingSection>
		<SettingSection id="instance-limits" icon="⇪" title={m.settings_sec_limits()} description={m.settings_limits_desc()} admin>
			{#if hasFeature('child_sessions')}
				<SpawnLimitsGroup />
			{/if}
			{#if hasFeature('uploads')}
				<UploadsSection {isAdmin} />
			{/if}
			<EventRetentionGroup />
		</SettingSection>
		<SettingSection id="instance-gateway" icon="⇆" title={m.settings_sec_gateway()} admin>
			<UpstreamHostsGroup />
		</SettingSection>
		{#if hasFeature('voice')}
			<SettingSection id="instance-integrations" icon="⚭" title={m.settings_sec_integrations()} admin>
				<SpeechGroup />
			</SettingSection>
		{/if}
		{#if hasFeature('plugins')}
			<SettingSection id="instance-plugins" icon="⧉" title={m.settings_nav_plugins()} admin>
				<PluginsAdminGroup {isAdmin} />
			</SettingSection>
		{/if}
		<ServerStorageSection />
		{#if hasFeature('observability')}
			<RedteamSection />
		{/if}
	</div>
{/if}

<style>
	.pg {
		display: none;
	}
	.pg.on {
		display: grid;
		gap: var(--sp-8);
	}
</style>
