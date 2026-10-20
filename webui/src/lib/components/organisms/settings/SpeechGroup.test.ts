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
	tts_model: 'kokoro',
	tts_voice: 'af_heart',
	tts_format: 'opus'
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
		type(input('Voice'), 'bf_emma');
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

	it('fills the pickers from the catalog and plays the test sample', async () => {
		api.speechSettings.mockResolvedValue(info());
		api.speechCatalog.mockResolvedValue({ models: ['kokoro', 'parakeet'], voices: ['af_heart', 'bf_emma'] });
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

		button('Load models and voices')?.click();
		await settle();
		const voices = [...document.querySelectorAll('#speech-voices option')].map((o) => o.getAttribute('value'));
		expect(voices).toEqual(['af_heart', 'bf_emma']);
		expect(document.querySelectorAll('#speech-models option')).toHaveLength(2);

		button('Test')?.click();
		await settle();
		expect(play).toHaveBeenCalled();
		expect(toast.ok).toHaveBeenCalledWith('Speech service is working');
		vi.unstubAllGlobals();
	});

	it('surfaces a save error as a toast', async () => {
		api.speechSettings.mockResolvedValue(info());
		api.setSpeechSettings.mockRejectedValue(new Error('base_url is required to enable speech'));
		comp = mount(SpeechGroup, { target: document.body });
		await settle();
		type(input('Base URL'), '');
		button('Save')?.click();
		await settle();
		expect(toast.error).toHaveBeenCalledWith('base_url is required to enable speech');
	});
});
