<script lang="ts">
	import { Badge, Button, Input, Select, Switch } from '@dorsk/tsumikit';
	import SettingGroup from '#lib/components/molecules/SettingGroup.svelte';
	import SettingRow from '#lib/components/molecules/SettingRow.svelte';
	import { endpoints } from '#lib/queries/index.ts';
	import type { SpeechCatalog } from '@bindings/SpeechCatalog';
	import type { SpeechConfig } from '@bindings/SpeechConfig';
	import type { SpeechSettingsInfo } from '@bindings/SpeechSettingsInfo';
	import { toasts } from '#lib/toast.svelte.ts';
	import { ApiError } from '#lib/api.ts';
	import { m } from '#lib/paraglide/messages.js';
	import { lostControls, missingHalves, type SpeechControl } from './speech.logic.ts';

	const FORMATS = ['opus', 'mp3', 'aac', 'flac', 'wav', 'pcm'];

	let info = $state<SpeechSettingsInfo | null>(null);
	let draft = $state<SpeechConfig | null>(null);
	let keyDraft = $state('');
	let catalog = $state<SpeechCatalog>({
		models: [],
		stt_models: [],
		tts_models: [],
		text_models: [],
		voices: []
	});
	let busy = $state(false);
	let confirming = $state<SpeechControl[]>([]);

	const CONTROL_LABEL: Record<SpeechControl, () => string> = {
		mic: m.settings_speech_control_mic,
		voice_spawn: m.settings_speech_control_voice_spawn,
		voice_mode: m.settings_speech_control_voice_mode,
		read_aloud: m.settings_speech_control_read_aloud
	};
	const missing = $derived(info ? missingHalves(info.config) : []);

	function apply(next: SpeechSettingsInfo) {
		info = next;
		draft = { ...next.config };
		keyDraft = '';
		confirming = [];
	}

	$effect(() => {
		endpoints
			.speechSettings()
			.then((next) => {
				apply(next);
				if (next.config.enabled)
					endpoints
						.speechCatalog()
						.then((c) => (catalog = c))
						.catch(() => {});
			})
			.catch(() => {});
	});

	/** The catalog's picks for a field, keeping the saved value selectable. */
	const choices = (tagged: string[], current: string) => {
		const list = tagged.length ? tagged : catalog.models;
		return current && !list.includes(current) ? [current, ...list] : list;
	};
	const pickers = $derived(
		draft
			? ([
					['stt_model', m.settings_speech_stt_model(), choices(catalog.stt_models, draft.stt_model)],
					['tts_model', m.settings_speech_tts_model(), choices(catalog.tts_models, draft.tts_model)],
					['tts_voice', m.settings_speech_tts_voice(), choices(catalog.voices, draft.tts_voice)]
				] as const)
			: []
	);

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

	const save = (clearKey = false, confirmed = false) =>
		run(async () => {
			if (!draft || !info) return;
			const lost = confirmed ? [] : lostControls(info.config, draft);
			if (lost.length) {
				confirming = lost;
				return;
			}
			const key = keyDraft.trim();
			try {
				apply(
					await endpoints.setSpeechSettings({
						config: { ...draft, stt_language: draft.stt_language?.trim() || null },
						revision: info.revision,
						...(key ? { api_key: key } : {}),
						...(clearKey ? { clear_key: true } : {})
					})
				);
			} catch (e) {
				if (!(e instanceof ApiError && e.status === 409)) throw e;
				apply(await endpoints.speechSettings());
				toasts.error(m.settings_speech_stale());
				return;
			}
			toasts.ok(m.settings_speech_saved());
		});

	function keepCurrent() {
		if (info && draft) {
			const { enabled, base_url, stt_model, tts_model } = info.config;
			draft = { ...draft, enabled, base_url, stt_model, tts_model };
		}
		confirming = [];
	}

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
	const selectValue = (e: Event) => (e.currentTarget as HTMLSelectElement).value;
	const partMs = (e: Event) => {
		const seconds = parseFloat(inputValue(e));
		return Number.isFinite(seconds) ? Math.round(seconds * 1000) : null;
	};
</script>

<div id="speech">
	<SettingGroup title={m.settings_speech_label()}>
		{#if info && draft}
			{#each missing as half (half)}
				<p class="hint" data-testid="speech-hint-{half}">
					{half === 'stt' ? m.settings_speech_hint_stt() : m.settings_speech_hint_tts()}
				</p>
			{/each}
			<SettingRow label={m.settings_speech_enabled()} help={m.settings_speech_help()}>
				<Switch
					checked={draft.enabled}
					label={m.settings_speech_enabled()}
					size="sm"
					onclick={() => draft && (draft.enabled = !draft.enabled)}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_url()}>
				<Input
					value={draft.base_url}
					placeholder="https://speech.example/v1"
					aria-label={m.settings_speech_url()}
					oninput={(e: Event) => draft && (draft.base_url = inputValue(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_key()} wide selfLabelled>
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
			{#each pickers as [field, label, options] (field)}
				<SettingRow {label}>
					<Select
						value={draft[field]}
						aria-label={label}
						style="width:100%"
						onchange={(e: Event) => draft && (draft[field] = selectValue(e))}
					>
						{#if field === 'stt_model'}
							<option value="">{m.settings_speech_stt_off()}</option>
						{:else if field === 'tts_model'}
							<option value="">{m.settings_speech_tts_off()}</option>
						{/if}
						{#each options as id (id)}
							<option value={id}>{id}</option>
						{/each}
					</Select>
				</SettingRow>
			{/each}
			<SettingRow
				label={m.settings_speech_summary_model()}
				help={m.settings_speech_summary_model_help()}
			>
				<Select
					value={draft.summary_model}
					aria-label={m.settings_speech_summary_model()}
					style="width:100%"
					onchange={(e: Event) => draft && (draft.summary_model = selectValue(e))}
				>
					<option value="">{m.settings_speech_summary_off()}</option>
					{#each choices(catalog.text_models, draft.summary_model) as id (id)}
						<option value={id}>{id}</option>
					{/each}
				</Select>
			</SettingRow>
			<SettingRow label={m.settings_speech_stt_language()}>
				<Input
					value={draft.stt_language ?? ''}
					placeholder="en"
					aria-label={m.settings_speech_stt_language()}
					oninput={(e: Event) => draft && (draft.stt_language = inputValue(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_stt_part()} help={m.settings_speech_stt_part_help()}>
				<Input
					type="number"
					min="1"
					max="28"
					step="0.5"
					value={draft.stt_part_ms == null ? '' : draft.stt_part_ms / 1000}
					placeholder="3"
					aria-label={m.settings_speech_stt_part()}
					oninput={(e: Event) => draft && (draft.stt_part_ms = partMs(e))}
				/>
			</SettingRow>
			<SettingRow label={m.settings_speech_tts_format()}>
				<Select
					value={draft.tts_format}
					aria-label={m.settings_speech_tts_format()}
					onchange={(e: Event) => draft && (draft.tts_format = selectValue(e))}
				>
					{#each FORMATS as f (f)}
						<option value={f}>{f}</option>
					{/each}
				</Select>
			</SettingRow>
			{#if confirming.length}
				<div class="confirm" role="alertdialog" aria-label={m.settings_speech_confirm_save()}>
					<p>
						{m.settings_speech_confirm({
							controls: confirming.map((c) => CONTROL_LABEL[c]()).join(', ')
						})}
					</p>
					<div class="line">
						<Button variant="danger" disabled={busy} onclick={() => save(false, true)}>
							{m.settings_speech_confirm_save()}
						</Button>
						<Button variant="ghost" disabled={busy} onclick={keepCurrent}>{m.common_cancel()}</Button>
					</div>
				</div>
			{/if}
			<div class="line actions">
				<Button disabled={!dirty || busy || confirming.length > 0} onclick={() => save()}>
					{m.settings_speech_save()}
				</Button>
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
	.hint {
		margin: 0;
		color: var(--warn);
		font-size: var(--fs-sm);
	}
	.confirm {
		display: flex;
		flex-direction: column;
		gap: var(--sp-2);
		padding: var(--sp-3);
		border: 1px solid var(--danger);
		border-radius: var(--r-md);
	}
	.confirm p {
		margin: 0;
	}
</style>
