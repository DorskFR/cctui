import { describe, expect, it } from 'vitest';
import type { VoiceConfigInfo } from '@bindings/VoiceConfigInfo';
import { voiceCaps } from './capabilities';

const config = (stt: boolean, tts: boolean): VoiceConfigInfo => ({
	enabled: stt || tts,
	stt,
	tts,
	stt_model: stt ? 'parakeet' : '',
	stt_language: null,
	tts_model: tts ? 'kokoro' : '',
	tts_voice: 'af_heart',
	tts_format: 'opus',
	can_summarize: false
});

describe('voiceCaps', () => {
	it('is all off until the config loads', () => {
		expect(voiceCaps(undefined)).toEqual({ stt: false, tts: false, live: false });
	});

	it('reports each half on its own; voice mode needs both', () => {
		expect(voiceCaps(config(true, false))).toEqual({ stt: true, tts: false, live: false });
		expect(voiceCaps(config(false, true))).toEqual({ stt: false, tts: true, live: false });
		expect(voiceCaps(config(true, true))).toEqual({ stt: true, tts: true, live: true });
	});
});
