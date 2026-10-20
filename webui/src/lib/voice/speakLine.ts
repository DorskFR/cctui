/** Read one message aloud through `/voice/speak`; a second call with the
 *  same key while it plays stops it instead. */
import { api } from '$lib/api';
import type { VoiceSpeakRequest } from '@bindings/VoiceSpeakRequest';

interface Current {
	key: string;
	audio: HTMLAudioElement;
	url: string;
	onend: () => void;
}

let current: Current | null = null;

function stopCurrent() {
	const cur = current;
	if (!cur) return;
	current = null;
	cur.audio.pause();
	URL.revokeObjectURL(cur.url);
	cur.onend();
}

export async function speakText(
	key: string,
	text: string,
	opts: { voice?: string; speed?: number; onend?: () => void } = {}
): Promise<'started' | 'stopped'> {
	if (current?.key === key) {
		stopCurrent();
		return 'stopped';
	}
	stopCurrent();
	const body: VoiceSpeakRequest = { text, voice: opts.voice, speed: opts.speed };
	const url = URL.createObjectURL(await api.postBlobJson('/voice/speak', body));
	const audio = new Audio(url);
	audio.playbackRate = opts.speed ?? 1;
	const mine: Current = { key, audio, url, onend: opts.onend ?? (() => {}) };
	current = mine;
	audio.onended = () => {
		if (current === mine) stopCurrent();
	};
	audio.onerror = audio.onended;
	try {
		await audio.play();
	} catch (e) {
		if (current === mine) stopCurrent();
		throw e;
	}
	return 'started';
}
