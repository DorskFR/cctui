// @vitest-environment happy-dom
import { flushSync, mount, unmount } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

const query = vi.hoisted(() => ({ data: undefined as Record<string, unknown> | undefined }));
const voiceVoices = vi.hoisted(() => vi.fn(async () => ({ voices: ['af_heart'] })));
vi.mock('$lib/queries', () => ({ useVoiceConfig: () => query, endpoints: { voiceVoices } }));
vi.mock('$lib/voice/devices', () => ({ canPickSpeaker: () => true, listDevices: async () => [] }));

import VoiceSection from './VoiceSection.svelte';

let comp: ReturnType<typeof mount> | null = null;
afterEach(() => {
	if (comp) unmount(comp);
	comp = null;
	document.body.innerHTML = '';
	vi.clearAllMocks();
});

function render(stt: boolean, tts: boolean) {
	query.data = {
		enabled: stt || tts,
		stt,
		tts,
		stt_model: stt ? 'parakeet' : '',
		stt_language: null,
		tts_model: tts ? 'kokoro' : '',
		tts_voice: 'af_heart',
		tts_format: 'opus',
		can_summarize: false
	};
	comp = mount(VoiceSection, { target: document.body });
	flushSync();
	return document.body.textContent ?? '';
}

const STT_ROWS = ['Microphone', 'Voice input', 'Send dictation automatically'];
const TTS_ROWS = ['Auto-play voice notes', 'Speaker', 'Playback speed'];
const LIVE_ROWS = ['Interrupt by talking', 'Spoken reply style'];

describe('VoiceSection follows the speech capabilities', () => {
	it('renders nothing without a speech service', () => {
		expect(render(false, false).trim()).toBe('');
		expect(voiceVoices).not.toHaveBeenCalled();
	});

	it('hides the speech-to-text rows when STT is off', () => {
		const text = render(false, true);
		for (const row of STT_ROWS) expect(text, row).not.toContain(row);
		for (const row of LIVE_ROWS) expect(text, row).not.toContain(row);
		for (const row of TTS_ROWS) expect(text, row).toContain(row);
	});

	it('hides the text-to-speech rows when TTS is off', () => {
		const text = render(true, false);
		for (const row of TTS_ROWS) expect(text, row).not.toContain(row);
		for (const row of LIVE_ROWS) expect(text, row).not.toContain(row);
		for (const row of STT_ROWS) expect(text, row).toContain(row);
		expect(voiceVoices).not.toHaveBeenCalled();
	});

	it('shows the voice-mode rows with both halves on', () => {
		const text = render(true, true);
		for (const row of [...STT_ROWS, ...TTS_ROWS, ...LIVE_ROWS]) expect(text, row).toContain(row);
	});
});
