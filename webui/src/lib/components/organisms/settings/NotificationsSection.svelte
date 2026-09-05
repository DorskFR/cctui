<script lang="ts">
	// Settings › Notifications: browser notification when a session waits for
	// input, and its sound. Both go through the `notify` singleton so the header
	// bell and this section stay in step.
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings } from '$lib/settings.svelte';
	import { notify } from '$lib/notify.svelte';
	import { m } from '$lib/paraglide/messages';

	async function toggleNotify() {
		if (notify.enabled) notify.disable();
		else await notify.enable();
		settings.recordNotifyEnabled();
	}
</script>

<SettingSection id="notifications" icon="🔔" title={m.settings_notifications_title()}>
	<SettingGroup>
		<SettingRow label={m.settings_notify_input_label()} help={m.settings_notify_input_help()}>
			<Switch
				bind:checked={() => notify.enabled, () => void toggleNotify()}
				label={m.settings_notifications_title()}
				disabled={!notify.supported}
			/>
		</SettingRow>
		<SettingRow label={m.settings_sound_label()}>
			<Switch
				bind:checked={() => notify.sound, (v) => settings.setNotifySound(v)}
				label={m.settings_notification_sound_label()}
			/>
		</SettingRow>
	</SettingGroup>
</SettingSection>
