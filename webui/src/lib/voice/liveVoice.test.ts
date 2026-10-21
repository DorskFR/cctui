import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_VOICE } from '$lib/settings.svelte';
import { LiveVoice, voiceSocketUrl, type SocketLike } from './liveVoice.svelte';
import { type AudioLike, SpeechPlayer } from './player';
import type { VadCallbacks, VadHandle } from './vad';

vi.mock('$lib/config', () => ({ wsBase: () => 'ws://x/api/v1' }));

class FakeSocket implements SocketLike {
	binaryType: BinaryType = 'blob';
	readyState = 0;
	onopen: ((ev: Event) => void) | null = null;
	onclose: ((ev: CloseEvent) => void) | null = null;
	onerror: ((ev: Event) => void) | null = null;
	onmessage: ((ev: MessageEvent) => void) | null = null;
	sent: (string | ArrayBuffer)[] = [];
	closed = false;
	constructor(readonly url: string) {}
	send(data: string | ArrayBuffer) {
		this.sent.push(data);
	}
	close() {
		this.closed = true;
		this.readyState = 3;
	}
	open() {
		this.readyState = 1;
		this.onopen?.(new Event('open'));
	}
	frame(msg: object | ArrayBuffer) {
		const data = msg instanceof ArrayBuffer ? msg : JSON.stringify(msg);
		this.onmessage?.({ data } as MessageEvent);
	}
	drop() {
		this.readyState = 3;
		this.onclose?.({} as CloseEvent);
	}
	texts(): object[] {
		return this.sent.filter((s): s is string => typeof s === 'string').map((s) => JSON.parse(s));
	}
}

function harness(opts: { vadFails?: boolean } = {}) {
	let socket: FakeSocket | null = null;
	let cb: VadCallbacks | null = null;
	const vad = { start: vi.fn(), pause: vi.fn(), destroy: vi.fn() } satisfies VadHandle;
	const player = {
		playing: false,
		unlock: vi.fn(async () => {}),
		begin: vi.fn(),
		chunk: vi.fn(),
		end: vi.fn(),
		stop: vi.fn()
	};
	const release = vi.fn();
	const voice = new LiveVoice({
		connect: (url) => (socket = new FakeSocket(url)),
		vad: async (c) => {
			if (opts.vadFails) throw new Error('mic denied');
			cb = c;
			return vad;
		},
		player: player as unknown as SpeechPlayer,
		wakeLock: async () => release,
		prefs: () => ({ ...DEFAULT_VOICE, voice: 'af_sky', bargeIn: true })
	});
	const settle = async () => {
		for (let i = 0; i < 4; i++) await Promise.resolve();
		flushSync();
	};
	return {
		voice,
		vad,
		player,
		release,
		settle,
		get socket() {
			return socket as FakeSocket | null;
		},
		get cb() {
			return cb as VadCallbacks | null;
		}
	};
}

describe('LiveVoice', () => {
	beforeEach(() => vi.clearAllMocks());

	it('builds the voice socket url from the session', () => {
		expect(voiceSocketUrl('s 1')).toBe('ws://x/api/v1/ws/voice?session=s%201');
	});

	it('connects, sends the mode, arms the mic and follows server state', async () => {
		const h = harness();
		await h.voice.start('sess');
		expect(h.voice.status).toBe('connecting');
		expect(h.player.unlock).toHaveBeenCalled();
		expect(h.socket?.binaryType).toBe('arraybuffer');

		h.socket?.open();
		await h.settle();
		expect(h.socket?.texts()).toEqual([
			{
				type: 'mode',
				barge_in: true,
				spoken_style: 'brief',
				voice: 'af_sky',
				speed: 1,
				language: 'auto'
			}
		]);
		expect(h.vad.start).toHaveBeenCalled();
		expect(h.voice.status).toBe('listening');

		h.socket?.frame({ type: 'state', state: 'thinking' });
		expect(h.voice.status).toBe('thinking');
		h.socket?.frame({ type: 'transcript', text: 'hello there' });
		expect(h.voice.transcript).toBe('hello there');
	});

	it('shows warming up while the model loads and still sends an early utterance', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		h.socket?.frame({ type: 'state', state: 'warming' });
		await h.settle();
		expect(h.voice.status).toBe('warming');
		expect(h.vad.start).toHaveBeenCalled();
		h.cb?.onSpeechEnd(new Float32Array([0.1, 0.2]));
		expect(h.socket?.sent.some((s) => s instanceof ArrayBuffer)).toBe(true);
		h.socket?.frame({ type: 'state', state: 'listening' });
		expect(h.voice.status).toBe('listening');
	});

	it('opens with the picked language and switches it live', async () => {
		expect(voiceSocketUrl('s', 'fr')).toBe('ws://x/api/v1/ws/voice?session=s&language=fr');
		expect(voiceSocketUrl('s', 'auto')).toBe('ws://x/api/v1/ws/voice?session=s');
		const h = harness();
		h.voice.setLanguage('fr');
		await h.voice.start('sess');
		expect(h.socket?.url).toBe('ws://x/api/v1/ws/voice?session=sess&language=fr');
		h.socket?.open();
		await h.settle();
		expect(h.socket?.texts()).toMatchObject([{ type: 'mode', language: 'fr' }]);

		h.voice.setLanguage('fr');
		h.voice.setLanguage('en');
		h.voice.setLanguage('auto');
		expect(h.socket?.texts().slice(1)).toEqual([
			{ type: 'mode', language: 'en' },
			{ type: 'mode', language: 'auto' }
		]);
	});

	it('sends an utterance as pcm16 and routes spoken replies to the player', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();

		h.cb?.onLevel(0.4);
		expect(h.voice.level).toBe(0.4);
		h.cb?.onSpeechEnd(new Float32Array([0, 0.5, -0.5]));
		const bin = h.socket?.sent.find((s) => s instanceof ArrayBuffer) as ArrayBuffer;
		expect(bin.byteLength).toBe(6);
		expect(new Int16Array(bin)[1]).toBe(0x3fff);

		h.socket?.frame({ type: 'speak_start', line_id: '1-1', text: 'Hi.' });
		h.socket?.frame(new Uint8Array([1, 2, 3]).buffer);
		h.socket?.frame({ type: 'speak_end', line_id: '1-1' });
		expect(h.player.begin).toHaveBeenCalledWith('1-1');
		expect(h.player.chunk).toHaveBeenCalledWith(new Uint8Array([1, 2, 3]));
		expect(h.player.end).toHaveBeenCalledWith('1-1');
	});

	it('barges in when speech starts during a reply', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.socket?.frame({ type: 'state', state: 'speaking' });

		h.cb?.onSpeechStart();
		expect(h.player.stop).toHaveBeenCalled();
		expect(h.socket?.texts().at(-1)).toEqual({ type: 'interrupt' });

		h.socket?.frame({ type: 'state', state: 'listening' });
		h.player.stop.mockClear();
		h.cb?.onSpeechStart();
		expect(h.player.stop).not.toHaveBeenCalled();
	});

	it('drops leftover lines of a reply cut by a barge-in', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.socket?.frame({ type: 'speak_start', line_id: '1-1', text: 'One.' });
		h.socket?.frame({ type: 'state', state: 'speaking' });

		h.cb?.onSpeechStart();
		h.player.begin.mockClear();
		h.socket?.frame({ type: 'speak_start', line_id: '1-2', text: 'Two.' });
		expect(h.player.begin).not.toHaveBeenCalled();

		h.socket?.frame({ type: 'state', state: 'thinking' });
		h.socket?.frame({ type: 'speak_start', line_id: '2-1', text: 'New.' });
		expect(h.player.begin).toHaveBeenCalledWith('2-1');

		h.voice.interrupt();
		h.socket?.frame({ type: 'speak_start', line_id: '2-2', text: 'Stale.' });
		h.socket?.frame({ type: 'speak_start', line_id: '3-1', text: 'Fresh.' });
		expect(h.player.begin).toHaveBeenCalledTimes(2);
		expect(h.player.begin).toHaveBeenLastCalledWith('3-1');
	});

	it('a barge-in empties the real player queue and the cut reply never resumes', async () => {
		const played: string[] = [];
		const audios: AudioLike[] = [];
		const player = new SpeechPlayer({
			mime: () => 'audio/ogg',
			createAudio: () => {
				const a: AudioLike = {
					src: '',
					playbackRate: 1,
					onended: null,
					onerror: null,
					play: async () => {
						if (a.src.startsWith('blob:')) played.push(a.src);
					},
					pause: vi.fn()
				};
				audios.push(a);
				return a;
			},
			createUrl: (blob) => `blob:${blob.size}`,
			revokeUrl: () => {}
		});
		let socket: FakeSocket | null = null;
		let cb: VadCallbacks | null = null;
		const voice = new LiveVoice({
			connect: (url) => (socket = new FakeSocket(url)),
			vad: async (c) => {
				cb = c;
				return { start: vi.fn(), pause: vi.fn(), destroy: vi.fn() };
			},
			player,
			wakeLock: async () => () => {},
			prefs: () => ({ ...DEFAULT_VOICE, bargeIn: true })
		});
		await voice.start('sess');
		const sock = socket as unknown as FakeSocket;
		sock.open();
		for (let i = 0; i < 4; i++) await Promise.resolve();
		const line = (id: string, size: number) => {
			sock.frame({ type: 'speak_start', line_id: id, text: id });
			sock.frame(new Uint8Array(size).buffer);
			sock.frame({ type: 'speak_end', line_id: id });
		};
		line('1-1', 1);
		line('1-2', 2);
		line('1-3', 3);
		expect(played).toEqual(['blob:1']);

		(cb as unknown as VadCallbacks).onSpeechStart();
		expect(player.playing).toBe(false);
		line('1-4', 4);
		audios[0].onended?.(new Event('ended'));
		expect(played).toEqual(['blob:1']);

		line('2-1', 5);
		expect(played).toEqual(['blob:1', 'blob:5']);
		voice.stop();
	});

	it('mute pauses the vad and drops utterances', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.voice.toggleMute();
		expect(h.vad.pause).toHaveBeenCalled();
		h.cb?.onSpeechEnd(new Float32Array([0.1]));
		expect(h.socket?.sent.some((s) => s instanceof ArrayBuffer)).toBe(false);
		h.voice.toggleMute();
		expect(h.vad.start).toHaveBeenCalledTimes(2);
	});

	it('stop tears everything down and releases the wake lock', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.voice.stop();
		expect(h.voice.status).toBe('off');
		expect(h.socket?.closed).toBe(true);
		expect(h.vad.destroy).toHaveBeenCalled();
		expect(h.player.stop).toHaveBeenCalled();
		expect(h.release).toHaveBeenCalled();
	});

	it('a dropped socket goes back to off, a mic failure to error', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.socket?.drop();
		expect(h.voice.status).toBe('off');
		expect(h.vad.destroy).toHaveBeenCalled();

		const bad = harness({ vadFails: true });
		await bad.voice.start('sess');
		bad.socket?.open();
		await bad.settle();
		expect(bad.voice.status).toBe('error');
		expect(bad.voice.error).toBe('mic denied');
		expect(bad.socket?.closed).toBe(true);
	});

	it('surfaces server errors without leaving the loop', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.socket?.frame({ type: 'error', message: 'speech service unreachable' });
		expect(h.voice.error).toBe('speech service unreachable');
		expect(h.voice.status).toBe('listening');
	});

	it('releases a wake lock granted after the loop already stopped', async () => {
		let grant: (r: () => void) => void = () => {};
		const release = vi.fn();
		const voice = new LiveVoice({
			connect: (url) => new FakeSocket(url),
			vad: async () => ({ start: vi.fn(), pause: vi.fn(), destroy: vi.fn() }),
			player: { unlock: async () => {}, stop: vi.fn() } as unknown as SpeechPlayer,
			wakeLock: () => new Promise((r) => (grant = r)),
			prefs: () => DEFAULT_VOICE
		});
		const started = voice.start('sess');
		voice.stop();
		grant(release);
		await started;
		expect(release).toHaveBeenCalled();
	});

	it('destroys a microphone that opens after a stop and restart', async () => {
		const sockets: FakeSocket[] = [];
		const grants: ((v: VadHandle) => void)[] = [];
		const vads = [0, 1].map(() => ({ start: vi.fn(), pause: vi.fn(), destroy: vi.fn() }));
		const voice = new LiveVoice({
			connect: (url) => {
				const s = new FakeSocket(url);
				sockets.push(s);
				return s;
			},
			vad: () => new Promise((r) => grants.push(r)),
			player: { unlock: async () => {}, stop: vi.fn() } as unknown as SpeechPlayer,
			wakeLock: async () => () => {},
			prefs: () => DEFAULT_VOICE
		});
		await voice.start('sess');
		sockets[0].open();
		voice.stop();
		await voice.start('sess');
		sockets[1].open();
		grants[0](vads[0]);
		grants[1](vads[1]);
		for (let i = 0; i < 4; i++) await Promise.resolve();
		expect(vads[0].destroy).toHaveBeenCalled();
		expect(vads[0].start).not.toHaveBeenCalled();
		expect(vads[1].start).toHaveBeenCalled();
		voice.stop();
		expect(vads[1].destroy).toHaveBeenCalled();
	});
});
