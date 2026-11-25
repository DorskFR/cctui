import { untrack } from 'svelte';
import { useVoiceConfig } from '#lib/queries/index.ts';
import { settings } from '#lib/settings.svelte.ts';
import { toasts } from '#lib/toast.svelte.ts';
import { errMessage } from '#lib/api.ts';
import { LiveVoice } from '#lib/voice/liveVoice.svelte.ts';
import { SpeechPlayer } from '#lib/voice/player.ts';
import { speakText } from '#lib/voice/speakLine.ts';
import { voiceCaps } from '#lib/voice/capabilities.ts';
import { acquireWakeLock } from '#lib/voice/wakeLock.ts';
import { ttsMime } from '#lib/voice/mime.ts';
import type { Line } from './types';

/** Construct during component init: it reads query context and owns effects. */
export class PaneVoice {
	readonly #config = useVoiceConfig();
	readonly caps = $derived(voiceCaps(this.#config.data));
	readonly live: LiveVoice;
	speakingKey = $state<string | null>(null);
	readonly #id: () => string;

	constructor(id: () => string) {
		this.#id = id;
		this.live = new LiveVoice({
			connect: (url) => new WebSocket(url),
			vad: (cb) => import('#lib/voice/vad.ts').then((mod) => mod.createVad(cb, settings.voice.micId)),
			player: new SpeechPlayer({
				mime: () => ttsMime(this.#config.data?.tts_format ?? 'opus'),
				speed: () => settings.voice.speed,
				speaker: () => settings.voice.speakerId
			}),
			wakeLock: acquireWakeLock,
			prefs: () => settings.voice
		});
		$effect(() => {
			void this.#id();
			return () => this.live.stop();
		});
		$effect(() => {
			if (this.live.error) toasts.error(this.live.error);
		});
		$effect(() => {
			this.live.setLanguage(settings.voice.language);
		});
		$effect(() => {
			if (!this.caps.live) untrack(() => this.live.stop());
		});
	}

	toggleLive = () => {
		if (this.live.active) this.live.stop();
		else void this.live.start(this.#id());
	};

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
					if (this.speakingKey === key) this.speakingKey = null;
				}
			});
			this.speakingKey = outcome === 'started' ? key : null;
		} catch (e) {
			this.speakingKey = null;
			toasts.error(errMessage(e));
		}
	};
}
