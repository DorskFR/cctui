// @vitest-environment happy-dom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const { postForm, query, voice } = vi.hoisted(() => ({
	postForm: vi.fn(async () => ({ text: 'fix the login bug' })),
	query: { data: undefined as { stt: boolean; tts: boolean } | undefined },
	voice: { micId: null, language: 'auto', autoSend: false }
}));
vi.mock('$lib/api', () => ({ api: { postForm }, errMessage: (e: unknown) => String(e) }));
vi.mock('$lib/toast.svelte', () => ({ toasts: { error: vi.fn(), info: vi.fn() } }));
vi.mock('$lib/queries', () => ({ useVoiceConfig: () => query }));
vi.mock('$lib/settings.svelte', () => ({ settings: { voice } }));

import SpawnDictation from './SpawnDictation.svelte';

class FakeRecorder {
	state = 'inactive';
	mimeType = 'audio/webm';
	ondataavailable: ((e: { data: Blob }) => void) | null = null;
	onstop: (() => void) | null = null;
	static isTypeSupported = () => true;
	start() {
		this.state = 'recording';
	}
	stop() {
		this.state = 'inactive';
		this.ondataavailable?.({ data: new Blob(['x'.repeat(100)]) });
		this.onstop?.();
	}
}

let comp: ReturnType<typeof mount> | null = null;
let sf: { form: { prompt: string }; promptEl: null; busy: boolean; submit: ReturnType<typeof vi.fn> };
let submitted: string[] = [];

beforeEach(() => {
	vi.useFakeTimers();
	vi.stubGlobal('navigator', {
		mediaDevices: { getUserMedia: async () => ({ getTracks: () => [{ stop: vi.fn() }] }) }
	});
	vi.stubGlobal('MediaRecorder', FakeRecorder);
	postForm.mockClear();
	voice.autoSend = false;
	submitted = [];
	sf = {
		form: { prompt: '' },
		promptEl: null,
		busy: false,
		submit: vi.fn(async () => {
			submitted.push(sf.form.prompt.trim());
		})
	};
});

afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.useRealTimers();
	vi.unstubAllGlobals();
});

function render(caps: { stt: boolean; tts: boolean }) {
	query.data = caps;
	comp = mount(SpawnDictation, { target: document.body, props: { sf: sf as never } });
	flushSync();
}

const mic = () => document.querySelector<HTMLButtonElement>('button[aria-label^="Dictate"]');
const fire = (el: Element, type: string) => {
	el.dispatchEvent(new Event(type, { bubbles: true, cancelable: true }));
	flushSync();
};
const settle = async () => {
	for (let i = 0; i < 8; i++) await Promise.resolve();
	flushSync();
};
const tap = async () => {
	fire(mic()!, 'pointerdown');
	await settle();
	vi.advanceTimersByTime(80);
	fire(mic()!, 'pointerup');
	await settle();
};
const dictate = async () => {
	await tap();
	vi.advanceTimersByTime(3000);
	await tap();
};

describe('spawn prompt dictation', () => {
	it('renders no mic without speech-to-text, even with text-to-speech on', () => {
		render({ stt: false, tts: true });
		expect(mic()).toBeNull();
		expect(document.querySelector('[data-journey="spawn-dictation"]')).toBeNull();
	});

	it('renders the shared mic without a voice-mode toggle when STT is on', () => {
		render({ stt: true, tts: true });
		expect(mic()).not.toBeNull();
		expect(document.querySelector('[data-journey="voice-controls"]')).not.toBeNull();
		expect(document.querySelector('[aria-label="Live voice conversation with this session"]')).toBeNull();
	});

	it('lands the dictated transcript in the prompt', async () => {
		render({ stt: true, tts: false });
		sf.form.prompt = 'context';
		await dictate();
		expect(postForm).toHaveBeenCalledTimes(1);
		expect(sf.form.prompt.trim()).toBe('context\n\nfix the login bug');
		expect(sf.submit).not.toHaveBeenCalled();
	});

	it('submits the transcript as the session prompt with auto-send on', async () => {
		voice.autoSend = true;
		render({ stt: true, tts: false });
		await dictate();
		expect(sf.submit).toHaveBeenCalledTimes(1);
		expect(submitted).toEqual(['fix the login bug']);
	});
});
