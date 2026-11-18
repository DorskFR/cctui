import type { SpeechConfig } from '@bindings/SpeechConfig';
import type { VoiceConfigInfo } from '@bindings/VoiceConfigInfo';

export type SpeechControl = 'mic' | 'voice_spawn' | 'voice_mode' | 'read_aloud';
export type SpeechHalf = 'stt' | 'tts';

const halves = (c: SpeechConfig) => {
	const on = c.enabled && c.base_url.trim() !== '';
	return { stt: on && c.stt_model.trim() !== '', tts: on && c.tts_model.trim() !== '' };
};

export function lostControls(saved: SpeechConfig, next: SpeechConfig): SpeechControl[] {
	const before = halves(saved);
	const after = halves(next);
	const lost: SpeechControl[] = [];
	if (before.stt && !after.stt) lost.push('mic', 'voice_spawn');
	if (before.tts && !after.tts) lost.push('read_aloud');
	if (before.stt && before.tts && !(after.stt && after.tts)) lost.push('voice_mode');
	return lost;
}

export function missingHalves(saved: SpeechConfig): SpeechHalf[] {
	if (!saved.enabled || saved.base_url.trim() === '') return [];
	const on = halves(saved);
	return (['stt', 'tts'] as const).filter((h) => !on[h]);
}

export function missingVoiceHalves(config: VoiceConfigInfo | null | undefined): SpeechHalf[] {
	if (!config?.enabled) return [];
	return (['stt', 'tts'] as const).filter((h) => !config[h]);
}
