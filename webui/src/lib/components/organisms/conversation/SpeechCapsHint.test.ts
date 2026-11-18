// @vitest-environment happy-dom
import { flushSync, mount, unmount, tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

const q = vi.hoisted(() => ({
	me: { data: { role: 'admin' } as { role: string } | undefined },
	voice: { data: undefined as Record<string, unknown> | undefined }
}));
vi.mock('#lib/queries/index.ts', () => ({
	useMe: () => q.me,
	useVoiceConfig: () => q.voice
}));

import SpeechCapsHint from './SpeechCapsHint.svelte';

const voice = (over: Record<string, unknown>) => ({
	enabled: true,
	stt: true,
	tts: true,
	stt_model: 'parakeet',
	stt_language: null,
	tts_model: 'kokoro',
	tts_voice: 'af_heart',
	tts_format: 'opus',
	can_summarize: false,
	...over
});

let comp: ReturnType<typeof mount> | null = null;
const drop = () => {
	if (comp) unmount(comp);
	comp = null;
};
afterEach(() => {
	drop();
	document.body.replaceChildren();
});

const render = async () => {
	comp = mount(SpeechCapsHint, { target: document.body });
	await tick();
	flushSync();
	return document.querySelector('[data-testid="speech-caps-hint"]');
};

describe('speech caps hint', () => {
	it('explains a hidden mic to an admin and links to the speech settings', async () => {
		q.me.data = { role: 'admin' };
		q.voice.data = voice({ stt: false, stt_model: '' });
		const el = await render();
		expect(el?.textContent).toContain('The mic is hidden for everyone');
		expect(el?.querySelector('a')?.getAttribute('href')).toBe('/settings/instance#speech');
	});

	it('explains missing read-aloud when only text-to-speech is off', async () => {
		q.me.data = { role: 'admin' };
		q.voice.data = voice({ tts: false, tts_model: '' });
		expect((await render())?.textContent).toContain('Read-aloud and voice mode are off');
	});

	it('stays hidden for non-admins, with speech off, or with both halves on', async () => {
		q.me.data = { role: 'user' };
		q.voice.data = voice({ stt: false });
		expect(await render()).toBeNull();
		drop();
		q.me.data = { role: 'admin' };
		q.voice.data = voice({ enabled: false, stt: false, tts: false });
		expect(await render()).toBeNull();
		drop();
		q.voice.data = voice({});
		expect(await render()).toBeNull();
	});

	it('dismisses for the rest of the page', async () => {
		q.me.data = { role: 'admin' };
		q.voice.data = voice({ stt: false });
		const el = await render();
		(el?.querySelector('button') as HTMLButtonElement).click();
		flushSync();
		expect(document.querySelector('[data-testid="speech-caps-hint"]')).toBeNull();
		drop();
		expect(await render()).toBeNull();
	});
});
