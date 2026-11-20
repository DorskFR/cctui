<script lang="ts">
	import { Switch } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import ThresholdInput from '#lib/components/molecules/ThresholdInput.svelte';
	import { settings } from '#lib/settings.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const setting = $derived(settings.autoFollowup);
</script>

{#snippet thresholdHelp()}
	{m.autofollowup_threshold_help()}
	{#if setting.threshold.unit === 'percent'}
		<span class="hint" data-testid="autofollowup-percent-hint">{m.autofollowup_percent_hint()}</span>
	{/if}
{/snippet}

<SettingGroup title={m.autofollowup_settings_title()}>
	<SettingRow
		label={m.autofollowup_enabled_label()}
		help={m.autofollowup_enabled_help()}
		journey="setting"
		journeyKey="auto-followup"
	>
		<Switch
			bind:checked={() => setting.enabled, (v) => settings.setAutoFollowup({ enabled: v })}
			label={m.autofollowup_enabled_label()}
		/>
	</SettingRow>
	<SettingRow
		label={m.autofollowup_threshold_label()}
		helpSlot={thresholdHelp}
		selfLabelled
	>
		<ThresholdInput
			value={setting.threshold}
			onchange={(threshold) => settings.setAutoFollowup({ threshold })}
		/>
	</SettingRow>
</SettingGroup>

<style>
	.hint {
		display: block;
		margin-top: var(--sp-1);
	}
</style>
