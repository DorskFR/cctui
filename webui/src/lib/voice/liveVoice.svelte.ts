/** The live voice loop for one session: microphone + VAD in, utterances up
 *  the voice socket, transcript and spoken reply back. The socket, VAD and
 *  player are injectable so the state machine runs under test. */
import type { VoiceClientMsg } from '@bindings/VoiceClientMsg';
import type { VoiceServerMsg } from '@bindings/VoiceServerMsg';
import { wsBase } from '$lib/config';
import type { VoiceSettings } from '$lib/settings.svelte';
import { floatToPcm16 } from './pcm';
import type { SpeechPlayer } from './player';
import type { VadFactory, VadHandle } from './vad';

export type LiveStatus = 'off' | 'connecting' | 'listening' | 'thinking' | 'speaking' | 'error';

export interface SocketLike {
	binaryType: BinaryType;
	readyState: number;
	onopen: ((ev: Event) => void) | null;
	onclose: ((ev: CloseEvent) => void) | null;
	onerror: ((ev: Event) => void) | null;
	onmessage: ((ev: MessageEvent) => void) | null;
	send(data: string | ArrayBuffer): void;
	close(): void;
}

export interface LiveVoiceDeps {
	connect: (url: string) => SocketLike;
	vad: VadFactory;
	player: SpeechPlayer;
	wakeLock: () => Promise<() => void>;
	prefs: () => VoiceSettings;
}

export function voiceSocketUrl(sessionId: string): string {
	return `${wsBase()}/ws/voice?session=${encodeURIComponent(sessionId)}`;
}

export class LiveVoice {
	status = $state<LiveStatus>('off');
	level = $state(0);
	muted = $state(false);
	transcript = $state('');
	error = $state<string | null>(null);

	private socket: SocketLike | null = null;
	private vad: VadHandle | null = null;
	private releaseWakeLock: (() => void) | null = null;
	private readonly deps: LiveVoiceDeps;

	constructor(deps: LiveVoiceDeps) {
		this.deps = deps;
	}

	get active(): boolean {
		return this.status !== 'off' && this.status !== 'error';
	}

	/** Must run from a user gesture: it unlocks audio for iOS. */
	async start(sessionId: string) {
		if (this.socket) return;
		this.error = null;
		this.transcript = '';
		this.status = 'connecting';
		void this.deps.player.unlock();
		const sock = this.deps.connect(voiceSocketUrl(sessionId));
		sock.binaryType = 'arraybuffer';
		this.socket = sock;
		sock.onopen = () => {
			if (this.socket !== sock) return;
			this.sendMode();
			void this.startMic(sock);
		};
		sock.onmessage = (ev) => {
			if (this.socket === sock) this.onMessage(ev.data);
		};
		sock.onerror = () => {
			if (this.socket === sock) this.fail('voice socket error');
		};
		sock.onclose = () => {
			if (this.socket !== sock) return;
			const wasActive = this.active;
			this.teardown();
			if (wasActive) this.status = 'off';
		};
		const release = await this.deps.wakeLock();
		if (this.socket === sock) this.releaseWakeLock = release;
		else release();
	}

	stop() {
		this.teardown();
		this.status = 'off';
	}

	toggleMute() {
		this.muted = !this.muted;
		if (this.muted) {
			this.vad?.pause();
			this.level = 0;
		} else void this.vad?.start();
	}

	/** The user interrupted by hand (the stop button while speaking). */
	interrupt() {
		this.deps.player.stop();
		this.send({ type: 'interrupt' });
	}

	private sendMode() {
		const p = this.deps.prefs();
		this.send({
			type: 'mode',
			barge_in: p.bargeIn,
			spoken_style: p.spokenStyle,
			voice: p.voice ?? null,
			speed: p.speed
		});
	}

	private async startMic(sock: SocketLike) {
		try {
			const vad = await this.deps.vad({
				onSpeechStart: () => this.onSpeechStart(),
				onSpeechEnd: (audio) => this.onSpeechEnd(audio),
				onLevel: (level) => {
					if (!this.muted) this.level = level;
				}
			});
			if (this.socket !== sock) {
				vad.destroy();
				return;
			}
			this.vad = vad;
			if (!this.muted) await vad.start();
			if (this.status === 'connecting') this.status = 'listening';
		} catch (e) {
			if (this.socket === sock) this.fail(e instanceof Error ? e.message : 'microphone unavailable');
		}
	}

	private onSpeechStart() {
		if (this.status === 'speaking' || this.deps.player.playing) {
			this.deps.player.stop();
			this.send({ type: 'interrupt' });
		}
	}

	private onSpeechEnd(audio: Float32Array) {
		if (!audio.length || this.muted) return;
		this.sendBinary(floatToPcm16(audio));
	}

	private onMessage(data: unknown) {
		if (data instanceof ArrayBuffer) {
			this.deps.player.chunk(new Uint8Array(data));
			return;
		}
		if (typeof data !== 'string') return;
		let msg: VoiceServerMsg;
		try {
			msg = JSON.parse(data);
		} catch {
			return;
		}
		switch (msg.type) {
			case 'state':
				this.status = msg.state;
				break;
			case 'transcript':
				this.transcript = msg.text;
				break;
			case 'speak_start':
				this.deps.player.begin(msg.line_id);
				break;
			case 'speak_end':
				this.deps.player.end(msg.line_id);
				break;
			case 'error':
				this.error = msg.message;
				break;
		}
	}

	private send(msg: VoiceClientMsg) {
		if (this.socket?.readyState === 1) this.socket.send(JSON.stringify(msg));
	}

	private sendBinary(buf: ArrayBuffer) {
		if (this.socket?.readyState === 1) this.socket.send(buf);
	}

	private fail(message: string) {
		this.error = message;
		this.teardown();
		this.status = 'error';
	}

	private teardown() {
		const sock = this.socket;
		this.socket = null;
		if (sock) {
			sock.onopen = null;
			sock.onmessage = null;
			sock.onerror = null;
			sock.onclose = null;
			sock.close();
		}
		this.vad?.destroy();
		this.vad = null;
		this.deps.player.stop();
		this.releaseWakeLock?.();
		this.releaseWakeLock = null;
		this.level = 0;
		this.muted = false;
	}
}
