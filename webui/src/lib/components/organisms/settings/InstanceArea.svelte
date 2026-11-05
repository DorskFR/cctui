<script lang="ts">
	// Settings › Instance (admins only): the deployment as a whole. Users and
	// dispatchers are screens rather than rows, so they mount only while this
	// area is shown.
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import AccessPanel from '$lib/components/organisms/access/AccessPanel.svelte';
	import DispatchersPanel from '$lib/components/organisms/DispatchersPanel.svelte';
	import EventRetentionGroup from './EventRetentionGroup.svelte';
	import HarnessUpdateGroup from './HarnessUpdateGroup.svelte';
	import InstanceSection from './InstanceSection.svelte';
	import MachineAdaptersGroup from './MachineAdaptersGroup.svelte';
	import PluginsAdminGroup from './PluginsAdminGroup.svelte';
	import SpawnLimitsGroup from './SpawnLimitsGroup.svelte';
	import SpeechGroup from './SpeechGroup.svelte';
	import UploadsSection from './UploadsSection.svelte';
	import UpstreamHostsGroup from './UpstreamHostsGroup.svelte';
	import { m } from '$lib/paraglide/messages';

	let { active = false }: { active?: boolean } = $props();
</script>

<InstanceSection />

<SettingSection id="users" icon="users" title={m.settings_nav_users()}>
	{#if active}
		<div data-journey="page" data-journey-key="users"><AccessPanel /></div>
	{/if}
</SettingSection>

<SettingSection id="dispatchers" icon="send" title={m.settings_nav_dispatchers()}>
	{#if active}
		<div data-journey="page" data-journey-key="dispatchers"><DispatchersPanel heading={false} /></div>
	{/if}
</SettingSection>

<SettingSection id="harnesses" icon="settings" title={m.set_harnesses_title()}>
	<HarnessUpdateGroup />
	<MachineAdaptersGroup isAdmin />
</SettingSection>

<SettingSection id="limits" icon="filter" title={m.set_limits_title()}>
	<SpawnLimitsGroup />
	<UploadsSection />
	<EventRetentionGroup />
</SettingSection>

<SettingSection id="network" icon="link" title={m.set_network_title()}>
	<UpstreamHostsGroup />
</SettingSection>

<SettingSection id="integrations" icon="volume-high" title={m.set_integrations_title()}>
	<SpeechGroup />
</SettingSection>

<SettingSection id="instance-plugins" icon="grid" title={m.set_instance_plugins_title()}>
	<PluginsAdminGroup isAdmin />
</SettingSection>
