<script lang="ts">
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { useNotificationSettings } from '$lib/queries';
	import { settings } from '$lib/settings.svelte';
	import { notify } from '$lib/notify.svelte';
	import { m } from '$lib/paraglide/messages';
	import ChannelSwitches from './notifications/ChannelSwitches.svelte';
	import EndpointsGroup from './notifications/EndpointsGroup.svelte';
	import QuietHoursGroup from './notifications/QuietHoursGroup.svelte';
	import RouteMatrix from './notifications/RouteMatrix.svelte';

	const resolved = useNotificationSettings();

	async function toggleNotify() {
		if (notify.enabled) notify.disable();
		else await notify.enable();
		settings.recordNotifyEnabled();
	}
</script>

<SettingSection id="notifications" icon="🔔" title={m.settings_notifications_title()}>
	<SettingGroup>
		<SettingRow label={m.notif_set_master_label()} help={m.notif_set_master_help()} server>
			<Switch
				bind:checked={
					() => settings.state.notifications.enabled,
					(v) => settings.setNotifications({ enabled: v })
				}
				label={m.notif_set_master_label()}
				data-notif-master
			/>
		</SettingRow>
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
	<ChannelSwitches resolved={resolved.data} />
	<RouteMatrix resolved={resolved.data} />
	<QuietHoursGroup />
	<EndpointsGroup />
</SettingSection>
