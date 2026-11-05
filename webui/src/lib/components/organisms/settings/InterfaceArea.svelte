<script lang="ts">
	// Settings › Interface: how cctui looks and is laid out on screen.
	import { Select, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import AppearanceSection from './AppearanceSection.svelte';
	import GuidesSection from './GuidesSection.svelte';
	import MonitoringSection from './MonitoringSection.svelte';
	import PanelsTable from './PanelsTable.svelte';
	import StorageSection from './StorageSection.svelte';
	import { settings, type NavPosition } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	const sl = $derived(settings.state.sessionList);
</script>

<AppearanceSection />

<SettingSection id="layout" icon="layout-grid" title={m.set_layout_title()}>
	<SettingGroup>
		<SettingRow label={m.settings_nav_position_label()} help={m.settings_nav_position_help()}>
			<Select
				value={settings.nav}
				style="width:100%"
				onchange={(e) => settings.setNav((e.currentTarget as HTMLSelectElement).value as NavPosition)}
			>
				<option value="top">{m.settings_nav_position_top()}</option>
				<option value="bottom">{m.settings_nav_position_bottom()}</option>
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_list_width_label()} help={m.settings_list_width_help()}>
			<Select
				value={sl.width}
				style="width:100%"
				onchange={(e) =>
					settings.setSessionList({
						width: (e.currentTarget as HTMLSelectElement).value as typeof sl.width
					})}
			>
				<option value="default">{m.settings_list_width_default()}</option>
				<option value="wide">{m.settings_list_width_wide()}</option>
				<option value="ultra">{m.settings_list_width_ultra()}</option>
				<option value="full">{m.settings_list_width_full()}</option>
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_account_names_label()} help={m.settings_account_names_help()}>
			<Switch
				bind:checked={() => sl.accountNames, (v) => settings.setSessionList({ accountNames: v })}
				label={m.settings_account_names_label()}
			/>
		</SettingRow>
	</SettingGroup>
	<SettingGroup title={m.set_panels_title()}>
		<PanelsTable />
	</SettingGroup>
</SettingSection>

<MonitoringSection />

<SettingSection id="conversation-display" icon="quote" title={m.set_conversation_display_title()}>
	<SettingGroup>
		<SettingRow
			label={m.settings_role_tinted_background_label()}
			help={m.settings_role_tinted_background_help()}
		>
			<Switch
				bind:checked={() => settings.roleTintedBackground, (v) => settings.setRoleTintedBackground(v)}
				label={m.settings_role_tinted_background_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_pin_first_message_label()} help={m.settings_pin_first_message_help()}>
			<Switch
				bind:checked={() => settings.pinFirstMessage, (v) => settings.setPinFirstMessage(v)}
				label={m.settings_pin_first_message_label()}
			/>
		</SettingRow>
	</SettingGroup>
</SettingSection>

<GuidesSection />

<StorageSection />
