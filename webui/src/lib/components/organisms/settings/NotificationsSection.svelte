<script lang="ts">
	// Notifications & Voice › Notifications: a browser notification when a
	// session waits for input, and its sound. One stored value in the settings
	// blob; the header menu toggles the same one.
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings } from '$lib/settings.svelte';
	import { notify } from '$lib/notify.svelte';
	import { m } from '$lib/paraglide/messages';
</script>

<SettingSection id="notifications" icon="🔔" title={m.settings_notifications_title()}>
	<SettingGroup>
		<SettingRow label={m.settings_notify_input_label()} help={m.settings_notify_input_help()}>
			<Switch
				bind:checked={() => notify.enabled, (v) => void settings.setNotifyEnabled(v)}
				label={m.settings_notify_input_label()}
				disabled={!notify.supported}
			/>
		</SettingRow>
		<SettingRow label={m.settings_notification_sound_label()} help={m.set_notification_sound_help()}>
			<Switch
				bind:checked={() => notify.sound, (v) => settings.setNotifySound(v)}
				label={m.settings_notification_sound_label()}
			/>
		</SettingRow>
	</SettingGroup>
</SettingSection>
