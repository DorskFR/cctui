<script lang="ts">
	import { Button, Select, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import SettingSection from '$lib/components/molecules/SettingSection.svelte';
	import {
		settings,
		type VoiceInputMode,
		type VoiceLanguage,
		type VoiceSpokenStyle
	} from '$lib/settings.svelte';
	import { type AudioDevice, canPickSpeaker, listDevices } from '$lib/voice/devices';
	import { m } from '$lib/paraglide/messages';
	import { endpoints, useVoiceConfig } from '$lib/queries';

	const SPEEDS = [0.75, 1, 1.25, 1.5, 2];

	const voiceConfig = useVoiceConfig();
	let voices = $state<string[]>([]);
	$effect(() => {
		if (!voiceConfig.data?.enabled) return;
		endpoints
			.voiceVoices()
			.then((v) => (voices = v.voices))
			.catch(() => {});
	});
	let mics = $state<AudioDevice[]>([]);
	let speakers = $state<AudioDevice[]>([]);
	const loadDevices = async () => {
		mics = await listDevices('audioinput');
		speakers = canPickSpeaker() ? await listDevices('audiooutput') : [];
	};
	/** Device names stay hidden until the page has used the microphone once. */
	const revealNames = async () => {
		const stream = await navigator.mediaDevices?.getUserMedia({ audio: true }).catch(() => null);
		for (const t of stream?.getTracks() ?? []) t.stop();
		await loadDevices();
	};
	$effect(() => {
		void loadDevices();
		navigator.mediaDevices?.addEventListener('devicechange', loadDevices);
		return () => navigator.mediaDevices?.removeEventListener('devicechange', loadDevices);
	});
	const unnamed = $derived(mics.some((d) => d.label.startsWith('🎙')));
	const LANGUAGES: [VoiceLanguage, () => string][] = [
		['auto', m.voice_live_language_auto],
		['en', m.voice_live_language_en],
		['fr', m.voice_live_language_fr]
	];
	const pick = (e: Event) => (e.currentTarget as HTMLSelectElement).value;

	const voiceChoices = $derived.by(() => {
		const current = settings.voice.voice;
		return current && !voices.includes(current) ? [current, ...voices] : voices;
	});
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
		<SettingRow label={m.settings_voice_mic_label()} help={m.settings_voice_mic_help()}>
			<div class="device">
				<Select
					value={settings.voice.micId ?? ''}
					style="width:100%"
					aria-label={m.settings_voice_mic_label()}
					onchange={(e: Event) => settings.setVoice({ micId: pick(e) })}
				>
					<option value="">{m.settings_voice_device_default()}</option>
					{#each mics as d (d.id)}
						<option value={d.id}>{d.label}</option>
					{/each}
				</Select>
				{#if unnamed}
					<Button variant="ghost" size="sm" onclick={revealNames}>
						{m.settings_voice_device_names()}
					</Button>
				{/if}
			</div>
		</SettingRow>
		{#if canPickSpeaker()}
			<SettingRow label={m.settings_voice_speaker_label()} help={m.settings_voice_speaker_help()}>
				<Select
					value={settings.voice.speakerId ?? ''}
					style="width:100%"
					aria-label={m.settings_voice_speaker_label()}
					onchange={(e: Event) => settings.setVoice({ speakerId: pick(e) })}
				>
					<option value="">{m.settings_voice_device_default()}</option>
					{#each speakers as d (d.id)}
						<option value={d.id}>{d.label}</option>
					{/each}
				</Select>
			</SettingRow>
		{/if}
		<SettingRow label={m.voice_live_language()} help={m.settings_voice_language_help()}>
			<Select
				value={settings.voice.language}
				style="width:100%"
				aria-label={m.voice_live_language()}
				onchange={(e: Event) => settings.setVoice({ language: pick(e) as VoiceLanguage })}
			>
				{#each LANGUAGES as [value, label] (value)}
					<option {value}>{label()}</option>
				{/each}
			</Select>
		</SettingRow>
		<SettingRow label={m.settings_voice_voice_label()} help={m.settings_voice_voice_help()}>
			<Select
				value={settings.voice.voice ?? ''}
				style="width:100%"
				aria-label={m.settings_voice_voice_label()}
				onchange={(e: Event) =>
					settings.setVoice({ voice: (e.currentTarget as HTMLSelectElement).value })}
			>
				<option value="">{m.settings_voice_voice_default()}</option>
				{#each voiceChoices as v (v)}
					<option value={v}>{v}</option>
				{/each}
			</Select>
		</SettingRow>
		{#if voiceConfig.data?.can_summarize}
			<SettingRow label={m.settings_voice_summarize_label()} help={m.settings_voice_summarize_help()}>
				<Switch
					bind:checked={
						() => settings.voice.summarizeLong,
						(v) => settings.setVoice({ summarizeLong: v })
					}
					label={m.settings_voice_summarize_label()}
				/>
			</SettingRow>
		{/if}
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

<style>
	.device {
		display: flex;
		gap: var(--sp-2);
		align-items: center;
		width: 100%;
	}
</style>
