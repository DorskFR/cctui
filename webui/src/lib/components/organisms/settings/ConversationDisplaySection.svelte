<script lang="ts">
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';
</script>

<SettingSection id="conversation-display" icon="❝" title={m.settings_sec_conversation_display()}>
	<SettingGroup>
		<SettingRow label={m.settings_role_tinted_background_label()} help={m.settings_role_tinted_background_help()}>
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
