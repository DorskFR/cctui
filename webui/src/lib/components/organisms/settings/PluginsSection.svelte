<script lang="ts">
	// Settings › Plugins: one switch per instance-enabled plugin. All off by
	// default; an enabled plugin adds its pane button to the conversation
	// drawer and its actions to assistant messages.
	import { Badge, EmptyState, Switch, Text } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import PluginSettingsForm from '$lib/components/molecules/PluginSettingsForm.svelte';
	import { settings } from '$lib/settings.svelte';
	import { usePlugins } from '$lib/queries';
	import { m } from '$lib/paraglide/messages';

	const plugins = usePlugins();
	const list = $derived(plugins.data ?? []);
</script>

<SettingSection id="plugins" icon="⧉" title={m.settings_nav_plugins()}>
	{#snippet descriptionSlot()}
		<Text size="sm" tone="faint">{m.settings_plugins_intro()}</Text>
	{/snippet}
	{#if plugins.isError}
		<EmptyState size="compact" tone="danger" icon="warning" title={m.settings_plugins_load_failed()} />
	{:else if plugins.isPending}
		<EmptyState size="compact" loading title={m.common_loading()} />
	{:else if list.length === 0}
		<EmptyState
			size="compact"
			icon="grid"
			title={m.settings_plugins_empty_title()}
			description={m.settings_plugins_empty_help()}
			data-journey="plugins-empty"
		/>
	{:else}
		<SettingGroup>
			{#each list as plugin (plugin.id)}
				<SettingRow label={plugin.name}>
					{#snippet helpSlot()}
						{plugin.description}
						<Text size="xs" tone="faint" data-journey="plugin-version">{plugin.version}</Text>
						{#if !plugin.web}
							<Badge size="xs">{m.settings_plugins_skills_only()}</Badge>
						{/if}
					{/snippet}
					<Switch
						bind:checked={() => settings.pluginsEnabled[plugin.id] === true, (v) => settings.setPluginEnabled(plugin.id, v)}
						label={plugin.name}
						data-journey="plugin-switch"
						data-plugin={plugin.id}
					/>
				</SettingRow>
				{#if settings.pluginsEnabled[plugin.id] === true}
					<PluginSettingsForm
						pluginId={plugin.id}
						decls={plugin.settings ?? []}
						values={settings.pluginConfig(plugin.id)}
						onchange={(key, value) => settings.setPluginConfig(plugin.id, key, value)}
					/>
				{/if}
			{/each}
		</SettingGroup>
	{/if}
</SettingSection>
