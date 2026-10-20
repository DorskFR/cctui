<script lang="ts">
	import { Badge, Button, Input, Select, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '$lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '$lib/components/molecules/SettingRow.svelte';
	import { endpoints } from '$lib/queries';
	import type { SpeechCatalog } from '@bindings/SpeechCatalog';
	import type { SpeechConfig } from '@bindings/SpeechConfig';
	import type { SpeechSettingsInfo } from '@bindings/SpeechSettingsInfo';
	import { toasts } from '$lib/toast.svelte';
	import { m } from '$lib/paraglide/messages';

	const FORMATS = ['opus', 'mp3', 'aac', 'flac', 'wav', 'pcm'];

	let info = $state<SpeechSettingsInfo | null>(null);
	let draft = $state<SpeechConfig | null>(null);
	let keyDraft = $state('');
	let catalog = $state<SpeechCatalog>({ models: [], voices: [] });
	let busy = $state(false);

	function apply(next: SpeechSettingsInfo) {
		info = next;
		draft = { ...next.config };
		keyDraft = '';
	}

	$effect(() => {
		endpoints
			.speechSettings()
			.then(apply)
			.catch(() => {});
	});

	const dirty = $derived(
		!!info && !!draft && (keyDraft.trim() !== '' || JSON.stringify(draft) !== JSON.stringify(info.config))
	);

	async function run(action: () => Promise<void>) {
		busy = true;
		try {
			await action();
		} catch (e) {
			toasts.error(e instanceof Error ? e.message : String(e));
		} finally {
			busy = false;
		}
	}

	const save = (clearKey = false) =>
		run(async () => {
			if (!draft) return;
			const key = keyDraft.trim();
			apply(
				await endpoints.setSpeechSettings({
					config: { ...draft, stt_language: draft.stt_language?.trim() || null },
					...(key ? { api_key: key } : {}),
					...(clearKey ? { clear_key: true } : {})
				})
			);
			toasts.ok(m.settings_speech_saved());
		});

	const loadCatalog = () =>
		run(async () => {
			catalog = await endpoints.speechCatalog();
		});

	const test = () =>
		run(async () => {
			const blob = await endpoints.testSpeech();
			const url = URL.createObjectURL(blob);
			const audio = new Audio(url);
			audio.onended = () => URL.revokeObjectURL(url);
			await audio.play().catch(() => {});
			toasts.ok(m.settings_speech_test_ok());
		});

	const inputValue = (e: Event) => (e.currentTarget as HTMLInputElement).value;
</script>

<div id="speech">
	<SettingGroup title={m.settings_speech_label()}>
		{#if info && draft}
			<SettingRow label={m.settings_speech_enabled()} help={m.settings_speech_help()} server admin>
				<Switch
					checked={draft.enabled}
					label={m.settings_speech_enabled()}
					size="sm"
					onclick={() => draft && (draft.enabled = !draft.enabled)}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_url()} server admin>
				<Input
					value={draft.base_url}
					placeholder="https://speech.example/v1"
					aria-label={m.settings_speech_url()}
					oninput={(e: Event) => draft && (draft.base_url = inputValue(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_key()} server admin wide selfLabelled>
				<div class="line">
					<Badge size="sm">{info.has_key ? m.settings_speech_key_set() : m.settings_speech_key_unset()}</Badge>
					<Input
						type="password"
						autocomplete="off"
						grow
						bind:value={keyDraft}
						placeholder={m.settings_speech_key_placeholder()}
						aria-label={m.settings_speech_key()}
					/>
					{#if info.has_key}
						<Button variant="ghost" disabled={busy} onclick={() => save(true)}>
							{m.settings_speech_key_clear()}
						</Button>
					{/if}
				</div>
			</SettingRow>
			{#each [['stt_model', m.settings_speech_stt_model()], ['tts_model', m.settings_speech_tts_model()]] as const as [field, label] (field)}
				<SettingRow {label} server admin>
					<Input
						value={draft[field]}
						list="speech-models"
						aria-label={label}
						oninput={(e: Event) => draft && (draft[field] = inputValue(e))}
					/>
				</SettingRow>
			{/each}
			<SettingRow label={m.settings_speech_stt_language()} server admin>
				<Input
					value={draft.stt_language ?? ''}
					placeholder="en"
					aria-label={m.settings_speech_stt_language()}
					oninput={(e: Event) => draft && (draft.stt_language = inputValue(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_tts_voice()} server admin>
				<Input
					value={draft.tts_voice}
					list="speech-voices"
					aria-label={m.settings_speech_tts_voice()}
					oninput={(e: Event) => draft && (draft.tts_voice = inputValue(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_tts_format()} server admin>
				<Select
					value={draft.tts_format}
					aria-label={m.settings_speech_tts_format()}
					onchange={(e: Event) => draft && (draft.tts_format = (e.currentTarget as HTMLSelectElement).value)}
				>
					{#each FORMATS as f (f)}
						<option value={f}>{f}</option>
					{/each}
				</Select>
			</SettingRow>
			<datalist id="speech-models">
				{#each catalog.models as id (id)}<option value={id}></option>{/each}
			</datalist>
			<datalist id="speech-voices">
				{#each catalog.voices as id (id)}<option value={id}></option>{/each}
			</datalist>
			<div class="line actions">
				<Button disabled={!dirty || busy} onclick={() => save()}>{m.settings_speech_save()}</Button>
				<Button variant="ghost" disabled={busy || dirty || !info.config.enabled} onclick={loadCatalog}>
					{m.settings_speech_load()}
				</Button>
				<Button variant="ghost" disabled={busy || dirty || !info.config.enabled} onclick={test}>
					{m.settings_speech_test()}
				</Button>
			</div>
		{/if}
	</SettingGroup>
</div>

<style>
	.line {
		display: flex;
		align-items: center;
		gap: var(--sp-2);
		flex-wrap: wrap;
		min-width: 0;
	}
	.actions {
		justify-content: flex-end;
		padding-top: var(--sp-2);
	}
</style>
