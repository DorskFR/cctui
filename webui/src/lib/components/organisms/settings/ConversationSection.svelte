<script lang="ts">
	import { SegmentedControl, Switch } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import SettingSection from '#lib/components/molecules/SettingSection.svelte';
	import AutoFollowupGroup from './AutoFollowupGroup.svelte';
	import { settings } from '#lib/settings.svelte.ts';
	import { clampFollowupWhenCold } from '#lib/followup.ts';
	import { hasFeature } from '#lib/features.svelte.ts';
	import { m } from '#lib/paraglide/messages.js';

	const followupColdOptions: SegmentOption[] = [
		{ value: 'auto', label: m.settings_followup_when_cold_auto() },
		{ value: 'modal', label: m.settings_followup_when_cold_modal() },
		{ value: 'offer', label: m.settings_followup_when_cold_offer() },
		{ value: 'off', label: m.settings_followup_when_cold_off() }
	];
</script>

{#snippet archiveHelp()}
	{m.settings_archive_shortcut_help()}
{/snippet}

<SettingSection id="conversation" icon="◰" title={m.settings_group_conversation()}>
	<SettingGroup>
		{#if hasFeature('automation') && hasFeature('machine_spawn')}
			<SettingRow
				label={m.settings_followup_when_cold_label()}
				help={m.settings_followup_when_cold_help()}
				selfLabelled
			>
				<SegmentedControl
					options={followupColdOptions}
					label={m.settings_followup_when_cold_label()}
					control
					bind:value={
						() => settings.followupWhenCold,
						(v) => settings.setFollowupWhenCold(clampFollowupWhenCold(v))
					}
				/>
			</SettingRow>
			<SettingRow label={m.settings_prefer_followup_label()} help={m.settings_prefer_followup_help()}>
				<Switch
					bind:checked={() => settings.preferFollowupOverFork, (v) => settings.setPreferFollowupOverFork(v)}
					label={m.settings_prefer_followup_label()}
				/>
			</SettingRow>
		{/if}
		{#if hasFeature('automation')}
			<SettingRow
				label={m.settings_auto_resume_label()}
				help={m.settings_auto_resume_help()}
				journey="setting"
				journeyKey="auto-resume"
			>
				<Switch
					bind:checked={() => settings.autoResumeOnConnectionLoss, (v) => settings.setAutoResumeOnConnectionLoss(v)}
					label={m.settings_auto_resume_label()}
				/>
			</SettingRow>
		{/if}
		<SettingRow label={m.settings_archive_shortcut_label()} helpSlot={archiveHelp}>
			<Switch
				bind:checked={() => settings.state.display.archiveShortcut, () => settings.toggleArchiveShortcut()}
				label={m.settings_archive_shortcut_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_archive_done_button_label()} help={m.settings_archive_done_button_help()}>
			<Switch
				bind:checked={() => settings.archiveDoneButton, (v) => settings.setArchiveDoneButton(v)}
				label={m.settings_archive_done_button_label()}
			/>
		</SettingRow>
	</SettingGroup>
	{#if hasFeature('automation') && hasFeature('machine_spawn')}
		<AutoFollowupGroup />
	{/if}
</SettingSection>
