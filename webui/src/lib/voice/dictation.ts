/** Push-to-talk dictation: record the microphone while held, transcribe
 *  the clip through `/voice/transcribe`. */
import { api } from '#lib/api.ts';
import type { VoiceTranscript } from '@bindings/VoiceTranscript';
import { micConstraints } from './devices';

const PREFERRED = ['audio/webm;codecs=opus', 'audio/webm', 'audio/mp4', 'audio/ogg;codecs=opus'];

export function pickMime(supported: (t: string) => boolean): string | undefined {
	return PREFERRED.find(supported);
}

export async function transcribeBlob(blob: Blob, language?: string): Promise<string> {
	const form = new FormData();
	form.append('file', blob, 'clip.webm');
	if (language) form.append('language', language);
	const res = await api.postForm<VoiceTranscript>('/voice/transcribe', form);
	return res.text.trim();
}

export const SPEECH_RMS = 0.02;
const METER_TICK_MS = 50;
/** Voiced ticks needed to count as speech, so a click's thump does not. */
const SPEECH_TICKS = 4;

export function rms(frame: Float32Array): number {
	let sum = 0;
	for (const v of frame) sum += v * v;
	return frame.length ? Math.sqrt(sum / frame.length) : 0;
}

interface Meter {
	ctx: AudioContext;
	timer: ReturnType<typeof setInterval>;
}

export class Dictation {
	private recorder: MediaRecorder | null = null;
	private meter: Meter | null = null;
	private unmetered = false;
	private voicedTicks = 0;
	private chunks: Blob[] = [];
	private stream: MediaStream | null = null;
	private wanted = false;
	private readonly device: () => { micId?: string; language?: string };

	constructor(device: () => { micId?: string; language?: string } = () => ({})) {
		this.device = device;
	}

	get recording(): boolean {
		return this.recorder?.state === 'recording';
	}

	/** Whether a voice was heard; always true where the level cannot be read. */
	get heard(): boolean {
		if (this.unmetered) return true;
		if (this.meter && this.meter.ctx.state !== 'running') return true;
		return this.voicedTicks >= SPEECH_TICKS;
	}

	/** A `stop` or `cancel` before the microphone opens wins: it never records. */
	async start() {
		if (this.recorder || this.wanted) return;
		this.wanted = true;
		this.voicedTicks = 0;
		let stream: MediaStream;
		try {
			stream = await navigator.mediaDevices.getUserMedia({
				audio: micConstraints(this.device().micId)
			});
		} catch (e) {
			this.wanted = false;
			throw e;
		}
		if (!this.wanted) {
			for (const t of stream.getTracks()) t.stop();
			return;
		}
		this.stream = stream;
		const mimeType = pickMime((t) => MediaRecorder.isTypeSupported(t));
		this.recorder = new MediaRecorder(this.stream, mimeType ? { mimeType } : undefined);
		this.chunks = [];
		this.recorder.ondataavailable = (e) => {
			if (e.data.size) this.chunks.push(e.data);
		};
		this.recorder.start();
		this.listen(stream);
	}

	private listen(stream: MediaStream) {
		const Ctx = globalThis.AudioContext;
		if (typeof Ctx !== 'function') {
			this.unmetered = true;
			return;
		}
		try {
			const ctx = new Ctx();
			const analyser = ctx.createAnalyser();
			analyser.fftSize = 1024;
			ctx.createMediaStreamSource(stream).connect(analyser);
			void ctx.resume().catch(() => {});
			const frame = new Float32Array(analyser.fftSize);
			const timer = setInterval(() => {
				analyser.getFloatTimeDomainData(frame);
				if (rms(frame) >= SPEECH_RMS) this.voicedTicks += 1;
			}, METER_TICK_MS);
			this.meter = { ctx, timer };
		} catch {
			this.unmetered = true;
		}
	}

	/** The transcript of what was recorded, empty when nothing was heard. */
	async stop(): Promise<string> {
		const rec = this.recorder;
		this.wanted = false;
		if (!rec) return '';
		this.recorder = null;
		const clip = await new Promise<Blob>((resolve) => {
			rec.onstop = () => resolve(new Blob(this.chunks, { type: rec.mimeType }));
			rec.stop();
		});
		this.release();
		return clip.size ? transcribeBlob(clip, this.device().language) : '';
	}

	cancel() {
		this.wanted = false;
		const rec = this.recorder;
		this.recorder = null;
		if (rec && rec.state !== 'inactive') rec.stop();
		this.release();
	}

	private release() {
		if (this.meter) {
			clearInterval(this.meter.timer);
			void this.meter.ctx.close().catch(() => {});
			this.meter = null;
		}
		for (const t of this.stream?.getTracks() ?? []) t.stop();
		this.stream = null;
		this.chunks = [];
	}
}
