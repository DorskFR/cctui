<script lang="ts">
	import { Switch } from '@dorsk/tsumikit';
	import type { NotificationSettings } from '@bindings/NotificationSettings';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { NOTIFICATION_CHANNELS, effectiveChannelEnabled } from '$lib/notificationPrefs';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';
	import { channelLabel } from './labels';

	let { resolved }: { resolved?: NotificationSettings } = $props();

	const prefs = $derived(settings.state.notifications);
</script>

<SettingGroup title={m.notif_set_channels_title()}>
	{#each NOTIFICATION_CHANNELS as channel (channel)}
		<SettingRow label={channelLabel(channel)} help={m.notif_set_channel_help()} server>
			<Switch
				label={channelLabel(channel)}
				disabled={!prefs.enabled}
				data-channel-switch={channel}
				bind:checked={
					() => effectiveChannelEnabled(prefs, resolved, channel),
					(v) => settings.setNotificationChannel(channel, v)
				}
			/>
		</SettingRow>
	{/each}
</SettingGroup>
