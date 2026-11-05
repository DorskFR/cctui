<script lang="ts">
	// Sessions › Conversation: how an open conversation behaves — follow-ups on
	// a cold cache, auto-resume, the archive shortcut and button.
	import { SegmentedControl, Switch } from '@dorsk/tsumikit';
	import type { SegmentOption } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import { settings } from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';
	import { clampFollowupWhenCold } from '$lib/followup';

	const followupColdOptions: SegmentOption[] = [
		{ value: 'auto', label: m.settings_followup_when_cold_auto() },
		{ value: 'modal', label: m.settings_followup_when_cold_modal() },
		{ value: 'offer', label: m.settings_followup_when_cold_offer() },
		{ value: 'off', label: m.settings_followup_when_cold_off() }
	];
</script>

<SettingSection id="conversation" icon="reply" title={m.settings_group_conversation()}>
	<SettingGroup>
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
		<SettingRow label={m.settings_archive_shortcut_label()} help={m.settings_archive_shortcut_help()}>
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
</SettingSection>
