import type { VoiceConfigInfo } from '@bindings/VoiceConfigInfo';

/** What the instance speech service can do; voice mode needs both halves. */
export type VoiceCaps = { stt: boolean; tts: boolean; live: boolean };

export function voiceCaps(config: VoiceConfigInfo | null | undefined): VoiceCaps {
	const stt = config?.stt === true;
	const tts = config?.tts === true;
	return { stt, tts, live: stt && tts };
}
