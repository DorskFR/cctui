// @vitest-environment happy-dom
import { flushSync, mount, unmount, tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SpeechSettingsInfo } from '@bindings/SpeechSettingsInfo';

const api = vi.hoisted(() => ({
	speechSettings: vi.fn(),
	setSpeechSettings: vi.fn(),
	speechCatalog: vi.fn(),
	testSpeech: vi.fn()
}));
const toast = vi.hoisted(() => ({ ok: vi.fn(), error: vi.fn() }));
vi.mock('$lib/queries', () => ({ endpoints: api }));
vi.mock('$lib/toast.svelte', () => ({ toasts: toast }));

import SpeechGroup from './SpeechGroup.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.replaceChildren();
	vi.clearAllMocks();
});

const settle = async () => {
	for (let i = 0; i < 5; i++) await tick();
	flushSync();
};
const button = (text: string) =>
	[...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === text) as
		| HTMLButtonElement
		| undefined;
const input = (label: string) =>
	document.querySelector<HTMLInputElement>(`input[aria-label="${label}"]`) as HTMLInputElement;
const select = (label: string) =>
	document.querySelector<HTMLSelectElement>(`select[aria-label="${label}"]`) as HTMLSelectElement;
const optionsOf = (label: string) => [...select(label).options].map((o) => o.value);
const choose = (label: string, value: string) => {
	const el = select(label);
	el.value = value;
	el.dispatchEvent(new Event('change', { bubbles: true }));
	flushSync();
};
const type = (el: HTMLInputElement, value: string) => {
	el.value = value;
	el.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
};

const config = {
	enabled: true,
	base_url: 'http://speech-router.ai:8000/v1',
	stt_model: 'parakeet',
	stt_language: null,
	stt_part_ms: null,
	tts_model: 'kokoro',
	tts_voice: 'af_heart',
	tts_format: 'opus',
	summary_model: ''
};
const catalog = {
	models: ['parakeet', 'whistle', 'kokoro', 'qwen3-1.7b'],
	stt_models: ['parakeet', 'whistle'],
	tts_models: ['kokoro'],
	text_models: ['qwen3-1.7b'],
	voices: ['af_heart', 'bf_emma']
};
const info = (over: Partial<SpeechSettingsInfo> = {}): SpeechSettingsInfo => ({
	config,
	has_key: false,
	source: 'settings',
	...over
});

describe('speech settings', () => {
	it('sends a new key write-only, keeps it otherwise, and clears it explicitly', async () => {
		api.speechSettings.mockResolvedValue(info());
		api.speechCatalog.mockResolvedValue(catalog);
		comp = mount(SpeechGroup, { target: document.body });
		await settle();
		expect(document.body.textContent).toContain('no key');
		expect(input('Base URL').value).toBe(config.base_url);
		expect(button('Save')?.disabled).toBe(true);

		api.setSpeechSettings.mockResolvedValue(info({ has_key: true }));
		type(input('API key'), ' sk-new ');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenCalledWith({ config, api_key: 'sk-new' });
		expect(document.body.textContent).toContain('key set');
		expect(input('API key').value).toBe('');

		api.setSpeechSettings.mockResolvedValue(info({ has_key: true, config: { ...config, tts_voice: 'bf_emma' } }));
		choose('Voice', 'bf_emma');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith({ config: { ...config, tts_voice: 'bf_emma' } });

		api.setSpeechSettings.mockResolvedValue(info());
		button('Clear key')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith(
			expect.objectContaining({ clear_key: true })
		);
		expect(document.body.textContent).toContain('no key');
	});

	it('offers each picker the catalog models of its role and plays the test sample', async () => {
		api.speechSettings.mockResolvedValue(info({ config: { ...config, tts_voice: 'custom' } }));
		api.speechCatalog.mockResolvedValue(catalog);
		api.testSpeech.mockResolvedValue(new Blob([new Uint8Array([1, 2])], { type: 'audio/ogg' }));
		const play = vi.fn().mockResolvedValue(undefined);
		vi.stubGlobal(
			'Audio',
			vi.fn(function (this: { play: typeof play }) {
				this.play = play;
			})
		);
		comp = mount(SpeechGroup, { target: document.body });
		await settle();

		expect(optionsOf('Speech-to-text model')).toEqual(['', 'parakeet', 'whistle']);
		expect(optionsOf('Text-to-speech model')).toEqual(['', 'kokoro']);
		expect(optionsOf('Voice')).toEqual(['custom', 'af_heart', 'bf_emma']);
		expect(optionsOf('Summary model')).toEqual(['', 'qwen3-1.7b']);

		api.setSpeechSettings.mockResolvedValue(info());
		choose('Speech-to-text model', 'whistle');
		choose('Summary model', 'qwen3-1.7b');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith({
			config: { ...config, tts_voice: 'custom', stt_model: 'whistle', summary_model: 'qwen3-1.7b' }
		});

		button('Test')?.click();
		await settle();
		expect(play).toHaveBeenCalled();
		expect(toast.ok).toHaveBeenCalledWith('Speech service is working');
		vi.unstubAllGlobals();
	});

	it('falls back to every model when the service tags none', async () => {
		api.speechSettings.mockResolvedValue(info());
		api.speechCatalog.mockResolvedValue({
			...catalog,
			stt_models: [],
			tts_models: [],
			text_models: []
		});
		comp = mount(SpeechGroup, { target: document.body });
		await settle();
		expect(optionsOf('Speech-to-text model')).toEqual(['', ...catalog.models]);
	});

	it('turns speech-to-text and text-to-speech off separately with an empty model', async () => {
		api.speechSettings.mockResolvedValue(info());
		api.speechCatalog.mockResolvedValue(catalog);
		comp = mount(SpeechGroup, { target: document.body });
		await settle();

		api.setSpeechSettings.mockResolvedValue(info({ config: { ...config, stt_model: '' } }));
		choose('Speech-to-text model', '');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith({ config: { ...config, stt_model: '' } });

		api.setSpeechSettings.mockResolvedValue(info({ config: { ...config, tts_model: '' } }));
		choose('Speech-to-text model', 'parakeet');
		choose('Text-to-speech model', '');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith({ config: { ...config, tts_model: '' } });
	});

	it('edits the part length in seconds and saves blank as the default', async () => {
		api.speechSettings.mockResolvedValue(info({ config: { ...config, stt_part_ms: 5000 } }));
		api.speechCatalog.mockResolvedValue(catalog);
		comp = mount(SpeechGroup, { target: document.body });
		await settle();
		const field = input('Text appears after (seconds of speech)');
		expect(field.value).toBe('5');

		api.setSpeechSettings.mockResolvedValue(info({ config: { ...config, stt_part_ms: 1500 } }));
		type(field, '1.5');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith({ config: { ...config, stt_part_ms: 1500 } });

		api.setSpeechSettings.mockResolvedValue(info());
		type(input('Text appears after (seconds of speech)'), '');
		button('Save')?.click();
		await settle();
		expect(api.setSpeechSettings).toHaveBeenLastCalledWith({ config });
	});

	it('surfaces a save error as a toast', async () => {
		api.speechSettings.mockResolvedValue(info());
		api.speechCatalog.mockResolvedValue(catalog);
		api.setSpeechSettings.mockRejectedValue(new Error('base_url is required to enable speech'));
		comp = mount(SpeechGroup, { target: document.body });
		await settle();
		type(input('Base URL'), '');
		button('Save')?.click();
		await settle();
		expect(toast.error).toHaveBeenCalledWith('base_url is required to enable speech');
	});
});
