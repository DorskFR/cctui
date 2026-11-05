import { errMessage } from '$lib/api';
import { settings } from '$lib/settings.svelte';
import { toasts } from '$lib/toast.svelte';
import { speakText } from '$lib/voice/speakLine';
import type { Line } from './types';

export class LineSpeaker {
	key = $state<string | null>(null);

	speak = async (ln: Line) => {
		if (!ln.key || !ln.text) return;
		const key = ln.key;
		try {
			const outcome = await speakText(key, ln.text, {
				voice: settings.voice.voice,
				speed: settings.voice.speed,
				summarize: settings.voice.summarizeLong,
				speaker: settings.voice.speakerId,
				onend: () => {
					if (this.key === key) this.key = null;
				}
			});
			this.key = outcome === 'started' ? key : null;
		} catch (e) {
			this.key = null;
			toasts.error(errMessage(e));
		}
	};
}
