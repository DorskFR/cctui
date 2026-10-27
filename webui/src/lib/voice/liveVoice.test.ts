import { flushSync } from 'svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_VOICE } from '$lib/settings.svelte';
import {
	ECHO_TAIL_MS,
	LiveVoice,
	REPLY_GRACE_MS,
	voiceSocketUrl,
	type SocketLike
} from './liveVoice.svelte';
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

/** Each sent frame as its control type or its binary byte length. */
function kinds(sent: (string | ArrayBuffer)[]): (string | number)[] {
	return sent.map((s) => (typeof s === 'string' ? JSON.parse(s).type : s.byteLength));
}

/** One short utterance as the VAD reports it: a frame, then speech start and end. */
function speak(h: { cb: VadCallbacks | null }, samples: number[]) {
	h.cb?.onFrame(new Float32Array(samples), 0.9);
	h.cb?.onSpeechStart();
	h.cb?.onSpeechEnd();
}

function harness(opts: { vadFails?: boolean; bargeIn?: boolean; language?: 'auto' | 'en' | 'fr' } = {}) {
	let socket: FakeSocket | null = null;
	let cb: VadCallbacks | null = null;
	let idle: (idle: boolean) => void = () => {};
	const vad = { start: vi.fn(), pause: vi.fn(), destroy: vi.fn() } satisfies VadHandle;
	const player = {
		playing: false,
		unlock: vi.fn(async () => {}),
		begin: vi.fn(),
		chunk: vi.fn(),
		end: vi.fn(),
		stop: vi.fn(),
		onIdleChange: vi.fn((fn: (idle: boolean) => void) => {
			idle = fn;
			return () => {};
		})
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
		prefs: () => ({
			...DEFAULT_VOICE,
			voice: 'af_sky',
			bargeIn: opts.bargeIn ?? true,
			language: opts.language ?? 'auto'
		}),
		chunker: { sampleRate: 1000, minPartMs: 100, pauseMs: 20, maxPartMs: 300, preRollMs: 50 }
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
		},
		idle: (v: boolean) => idle(v)
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
		speak(h, [0.1, 0.2]);
		expect(h.socket?.sent.some((s) => s instanceof ArrayBuffer)).toBe(true);
		h.socket?.frame({ type: 'state', state: 'listening' });
		expect(h.voice.status).toBe('listening');
	});

	it('opens with the language from settings and switches it live', async () => {
		expect(voiceSocketUrl('s', 'fr')).toBe('ws://x/api/v1/ws/voice?session=s&language=fr');
		expect(voiceSocketUrl('s', 'auto')).toBe('ws://x/api/v1/ws/voice?session=s');
		const h = harness({ language: 'fr' });
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

		speak(h, [0, 0.5, -0.5]);
		expect(h.voice.level).toBe(1);
		const bin = h.socket?.sent.find((s) => s instanceof ArrayBuffer) as ArrayBuffer;
		expect(bin.byteLength).toBe(6);
		expect(new Int16Array(bin)[1]).toBe(0x3fff);
		expect(h.socket?.texts().some((m) => (m as { type: string }).type === 'part')).toBe(false);

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

	it('without barge-in, ignores what the mic hears over a reply and its echo tail', async () => {
		vi.useFakeTimers();
		const h = harness({ bargeIn: false });
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.socket?.frame({ type: 'state', state: 'speaking' });
		h.idle(false);
		h.player.playing = true;
		const before = h.socket?.sent.length;

		speak(h, [0.5, 0.5, 0.5]);
		expect(h.player.stop).not.toHaveBeenCalled();
		expect(h.socket?.sent.length).toBe(before);

		h.player.playing = false;
		h.idle(true);
		h.socket?.frame({ type: 'state', state: 'listening' });
		speak(h, [0.5, 0.5, 0.5]);
		expect(h.socket?.sent.length).toBe(before);

		vi.advanceTimersByTime(ECHO_TAIL_MS + 1);
		speak(h, [0.5, 0.5, 0.5]);
		expect(h.socket?.sent.some((s) => s instanceof ArrayBuffer)).toBe(true);
		expect(h.socket?.texts().some((m) => (m as { type: string }).type === 'interrupt')).toBe(false);
		vi.useRealTimers();
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

	it('streams a long utterance as marked parts and a bare final binary', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		const before = h.socket!.sent.length;
		const frame = (n: number, v: number, prob: number) =>
			h.cb?.onFrame(new Float32Array(n).fill(v), prob);
		frame(10, 0.1, 0.1);
		h.cb?.onSpeechStart();
		for (let i = 0; i < 6; i++) frame(50, 0.2, 0.9);
		frame(20, 0, 0.1);
		frame(30, 0.3, 0.9);
		h.cb?.onSpeechEnd();
		const sent = h.socket!.sent.slice(before);
		expect(kinds(sent)).toEqual(['part', 2 * 310, 2 * 50]);
		const first = new Int16Array(sent[1] as ArrayBuffer);
		expect(first[0]).toBe(Math.floor(0.1 * 0x7fff));
		expect(first[10]).toBe(Math.floor(0.2 * 0x7fff));
	});

	it('closes a streamed utterance with utterance_end when speech ends on a flush', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		const before = h.socket!.sent.length;
		h.cb?.onSpeechStart();
		for (let i = 0; i < 6; i++) h.cb?.onFrame(new Float32Array(50).fill(0.2), 0.9);
		h.cb?.onSpeechEnd();
		const sent = h.socket!.sent.slice(before);
		expect(kinds(sent)).toEqual(['part', 600, 'utterance_end']);
	});

	it('a misfire sends nothing and leaves nothing for the next utterance', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		const before = h.socket!.sent.length;
		h.cb?.onFrame(new Float32Array(5).fill(0.5), 0.9);
		h.cb?.onSpeechStart();
		h.cb?.onMisfire();
		expect(h.socket!.sent.length).toBe(before);
		speak(h, [0.1, 0.1]);
		const bin = h.socket!.sent.at(-1) as ArrayBuffer;
		expect(bin.byteLength).toBe(4);
	});

	it('mute pauses the vad and drops utterances', async () => {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		h.voice.toggleMute();
		expect(h.vad.pause).toHaveBeenCalled();
		speak(h, [0.1]);
		expect(h.socket?.sent.some((s) => s instanceof ArrayBuffer)).toBe(false);
		expect(h.voice.level).toBe(0);
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
			player: { unlock: async () => {}, stop: vi.fn(), onIdleChange: () => () => {} } as unknown as SpeechPlayer,
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
			player: { unlock: async () => {}, stop: vi.fn(), onIdleChange: () => () => {} } as unknown as SpeechPlayer,
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

describe('LiveVoice shown state', () => {
	beforeEach(() => vi.clearAllMocks());

	async function talking() {
		const h = harness();
		await h.voice.start('sess');
		h.socket?.open();
		await h.settle();
		return h;
	}

	function reply(h: Awaited<ReturnType<typeof talking>>, ids: string[]) {
		h.socket?.frame({ type: 'state', state: 'speaking' });
		for (const id of ids) {
			h.socket?.frame({ type: 'speak_start', line_id: id, text: id });
			h.socket?.frame(new Uint8Array([1]).buffer);
			h.socket?.frame({ type: 'speak_end', line_id: id });
			if (id === ids[0]) h.idle(false);
		}
		h.socket?.frame({ type: 'state', state: 'listening' });
	}

	it('stays speaking while queued audio plays after the server says listening', async () => {
		const h = await talking();
		reply(h, ['1-1', '1-2', '1-3']);
		expect(h.voice.status).toBe('speaking');
		h.idle(true);
		expect(h.voice.status).toBe('listening');
	});

	it('shows speaking over a server thinking while the reply still plays', async () => {
		const h = await talking();
		reply(h, ['1-1', '1-2']);
		h.socket?.frame({ type: 'state', state: 'thinking' });
		expect(h.voice.status).toBe('speaking');
		h.idle(true);
		expect(h.voice.status).toBe('thinking');
		h.socket?.frame({ type: 'state', state: 'listening' });
		expect(h.voice.status).toBe('listening');
	});

	it('holds thinking between the transcript and the first sentence', async () => {
		vi.useFakeTimers();
		try {
			const h = await talking();
			h.socket?.frame({ type: 'state', state: 'thinking' });
			h.socket?.frame({ type: 'transcript', text: 'why is the sky blue' });
			h.socket?.frame({ type: 'state', state: 'listening' });
			expect(h.voice.status).toBe('thinking');
			vi.advanceTimersByTime(REPLY_GRACE_MS - 1);
			expect(h.voice.status).toBe('thinking');
			reply(h, ['1-1']);
			expect(h.voice.status).toBe('speaking');
			h.idle(true);
			expect(h.voice.status).toBe('listening');
			vi.advanceTimersByTime(REPLY_GRACE_MS);
			expect(h.voice.status).toBe('listening');
		} finally {
			vi.useRealTimers();
		}
	});

	it('falls back to listening when no reply follows the transcript', async () => {
		vi.useFakeTimers();
		try {
			const h = await talking();
			h.socket?.frame({ type: 'transcript', text: 'hm' });
			h.socket?.frame({ type: 'state', state: 'listening' });
			expect(h.voice.status).toBe('thinking');
			vi.advanceTimersByTime(REPLY_GRACE_MS);
			expect(h.voice.status).toBe('listening');

			h.socket?.frame({ type: 'transcript', text: 'again' });
			h.socket?.frame({ type: 'state', state: 'listening' });
			h.socket?.frame({ type: 'error', message: 'agent failed' });
			expect(h.voice.status).toBe('listening');
		} finally {
			vi.useRealTimers();
		}
	});

	it('a barge-in during held playback interrupts and returns to listening', async () => {
		const h = await talking();
		reply(h, ['1-1', '1-2']);
		expect(h.voice.status).toBe('speaking');
		h.player.stop.mockImplementation(() => h.idle(true));
		h.cb?.onSpeechStart();
		expect(h.player.stop).toHaveBeenCalled();
		expect(h.socket?.texts().at(-1)).toEqual({ type: 'interrupt' });
		expect(h.voice.status).toBe('listening');
	});

	it('drains the real player queue before showing listening', async () => {
		const audios: AudioLike[] = [];
		const player = new SpeechPlayer({
			mime: () => 'audio/ogg',
			createAudio: () => {
				const a: AudioLike = {
					src: '',
					playbackRate: 1,
					onended: null,
					onerror: null,
					play: async () => {},
					pause: vi.fn()
				};
				audios.push(a);
				return a;
			},
			createUrl: (blob) => `blob:${blob.size}`,
			revokeUrl: () => {}
		});
		let socket: FakeSocket | null = null;
		const voice = new LiveVoice({
			connect: (url) => (socket = new FakeSocket(url)),
			vad: async () => ({ start: vi.fn(), pause: vi.fn(), destroy: vi.fn() }),
			player,
			wakeLock: async () => () => {},
			prefs: () => ({ ...DEFAULT_VOICE, bargeIn: true })
		});
		await voice.start('sess');
		const sock = socket as unknown as FakeSocket;
		sock.open();
		for (let i = 0; i < 4; i++) await Promise.resolve();
		sock.frame({ type: 'state', state: 'speaking' });
		for (const id of ['1-1', '1-2']) {
			sock.frame({ type: 'speak_start', line_id: id, text: id });
			sock.frame(new Uint8Array(2).buffer);
			sock.frame({ type: 'speak_end', line_id: id });
		}
		sock.frame({ type: 'state', state: 'listening' });
		expect(voice.status).toBe('speaking');
		audios[1].onended?.(new Event('ended'));
		expect(voice.status).toBe('speaking');
		audios[2].onended?.(new Event('ended'));
		expect(voice.status).toBe('listening');
		voice.stop();
	});
});
