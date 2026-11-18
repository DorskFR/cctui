import { describe, expect, it } from 'vitest';
import type { SpeechConfig } from '@bindings/SpeechConfig';
import { lostControls, missingHalves, missingVoiceHalves } from './speech.logic.ts';

const on: SpeechConfig = {
	enabled: true,
	base_url: 'http://speech.ai/v1',
	stt_model: 'parakeet',
	stt_language: null,
	stt_part_ms: null,
	tts_model: 'kokoro',
	tts_voice: 'af_heart',
	tts_format: 'opus',
	summary_model: ''
};

describe('speech blast radius', () => {
	it('names what each blanked half takes away from every user', () => {
		expect(lostControls(on, { ...on, stt_model: '' })).toEqual(['mic', 'voice_spawn', 'voice_mode']);
		expect(lostControls(on, { ...on, tts_model: ' ' })).toEqual(['read_aloud', 'voice_mode']);
		expect(lostControls(on, { ...on, enabled: false })).toEqual([
			'mic',
			'voice_spawn',
			'read_aloud',
			'voice_mode'
		]);
		expect(lostControls(on, { ...on, base_url: '' })).toHaveLength(4);
	});

	it('stays quiet when nothing that was on goes off', () => {
		expect(lostControls(on, { ...on, tts_voice: 'bf_emma' })).toEqual([]);
		const sttOff = { ...on, stt_model: '' };
		expect(lostControls(sttOff, sttOff)).toEqual([]);
		expect(lostControls(sttOff, on)).toEqual([]);
		expect(lostControls({ ...on, enabled: false }, { ...on, enabled: false, stt_model: '' })).toEqual([]);
	});

	it('flags a half that is off only while speech is on', () => {
		expect(missingHalves(on)).toEqual([]);
		expect(missingHalves({ ...on, stt_model: '' })).toEqual(['stt']);
		expect(missingHalves({ ...on, tts_model: '' })).toEqual(['tts']);
		expect(missingHalves({ ...on, enabled: false, stt_model: '' })).toEqual([]);
		expect(missingHalves({ ...on, base_url: '', stt_model: '' })).toEqual([]);
	});

	it('reads the same from the public voice config', () => {
		const voice = {
			enabled: true,
			stt: false,
			tts: true,
			stt_model: '',
			stt_language: null,
			tts_model: 'kokoro',
			tts_voice: 'af_heart',
			tts_format: 'opus',
			can_summarize: false
		};
		expect(missingVoiceHalves(voice)).toEqual(['stt']);
		expect(missingVoiceHalves({ ...voice, enabled: false })).toEqual([]);
		expect(missingVoiceHalves(undefined)).toEqual([]);
	});
});
