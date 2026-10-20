<script lang="ts">
	import { Input, Select, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import {
		settings,
		type VoiceInputMode,
		type VoiceSpokenStyle
	} from '$lib/settings.svelte';
	import { m } from '$lib/paraglide/messages';

	const SPEEDS = [0.75, 1, 1.25, 1.5, 2];
</script>

<SettingSection id="voice" icon="🔊" title={m.settings_voice_title()}>
	<SettingGroup>
		<SettingRow label={m.settings_voice_autoplay_label()} help={m.settings_voice_autoplay_help()}>
			<Switch
				bind:checked={
					() => settings.voice.autoPlayVoiceNotes,
					(v) => settings.setVoice({ autoPlayVoiceNotes: v })
				}
				label={m.settings_voice_autoplay_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_voice_voice_label()} help={m.settings_voice_voice_help()}>
			<Input
				value={settings.voice.voice ?? ''}
				placeholder={m.settings_voice_voice_default()}
				aria-label={m.settings_voice_voice_label()}
				onchange={(e: Event) =>
					settings.setVoice({ voice: (e.currentTarget as HTMLInputElement).value })}
			/>
		</SettingRow>
		<SettingRow label={m.settings_voice_speed_label()}>
			<Select
				value={String(settings.voice.speed)}
				style="width:100%"
				onchange={(e) =>
					settings.setVoice({ speed: Number((e.currentTarget as HTMLSelectElement).value) })}
			>
				{#each SPEEDS as s (s)}
					<option value={String(s)}>{s}×</option>
				{/each}
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_voice_input_mode_label()}>
			<Select
				value={settings.voice.inputMode}
				style="width:100%"
				onchange={(e) =>
					settings.setVoice({
						inputMode: (e.currentTarget as HTMLSelectElement).value as VoiceInputMode
					})}
			>
				<option value="push">{m.settings_voice_input_push()}</option>
				<option value="handsfree">{m.settings_voice_input_handsfree()}</option>
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_voice_autosend_label()} help={m.settings_voice_autosend_help()}>
			<Switch
				bind:checked={() => settings.voice.autoSend, (v) => settings.setVoice({ autoSend: v })}
				label={m.settings_voice_autosend_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_voice_bargein_label()} help={m.settings_voice_bargein_help()}>
			<Switch
				bind:checked={() => settings.voice.bargeIn, (v) => settings.setVoice({ bargeIn: v })}
				label={m.settings_voice_bargein_label()}
			/>
		</SettingRow>
		<SettingRow label={m.settings_voice_style_label()} help={m.settings_voice_style_help()}>
			<Select
				value={settings.voice.spokenStyle}
				style="width:100%"
				onchange={(e) =>
					settings.setVoice({
						spokenStyle: (e.currentTarget as HTMLSelectElement).value as VoiceSpokenStyle
					})}
			>
				<option value="brief">{m.settings_voice_style_brief()}</option>
				<option value="normal">{m.settings_voice_style_normal()}</option>
				<option value="verbose">{m.settings_voice_style_verbose()}</option>
			</Select>
		</SettingRow>
	</SettingGroup>
</SettingSection>
