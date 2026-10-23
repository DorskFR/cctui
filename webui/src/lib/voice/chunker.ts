/** Cuts one utterance into parts the voice socket transcribes while the
 *  user is still talking. VAD frames are the only source of samples, so
 *  parts neither overlap nor skip. */

export interface ChunkerOptions {
	sampleRate: number;
	minPartMs: number;
	pauseMs: number;
	/** Must stay under the server's per-chunk cap (30 s). */
	maxPartMs: number;
	preRollMs: number;
	speechThreshold: number;
}

export const CHUNKER_DEFAULTS: ChunkerOptions = {
	sampleRate: 16_000,
	minPartMs: 8_000,
	pauseMs: 400,
	maxPartMs: 28_000,
	preRollMs: 800,
	speechThreshold: 0.3
};

/** `samples` is empty only when `last`: the utterance ended after a flush. */
export type PartSink = (samples: Float32Array, last: boolean) => void;

export class UtteranceChunker {
	private readonly emit: PartSink;
	private readonly opts: ChunkerOptions;
	private preRoll: Float32Array[] = [];
	private preRollSamples = 0;
	private frames: Float32Array[] = [];
	private buffered = 0;
	private quiet = 0;
	private speaking = false;
	private sent = 0;

	constructor(emit: PartSink, opts: Partial<ChunkerOptions> = {}) {
		this.emit = emit;
		this.opts = { ...CHUNKER_DEFAULTS, ...opts };
	}

	start() {
		if (this.speaking) return;
		this.speaking = true;
		this.frames = this.preRoll;
		this.buffered = this.preRollSamples;
		this.preRoll = [];
		this.preRollSamples = 0;
		this.quiet = 0;
		this.sent = 0;
	}

	frame(samples: Float32Array, speechProb: number) {
		if (!samples.length) return;
		if (!this.speaking) {
			this.preRoll.push(samples);
			this.preRollSamples += samples.length;
			const max = this.samples(this.opts.preRollMs);
			while (this.preRoll.length > 1 && this.preRollSamples > max) {
				this.preRollSamples -= this.preRoll.shift()?.length ?? 0;
			}
			return;
		}
		this.frames.push(samples);
		this.buffered += samples.length;
		this.quiet = speechProb < this.opts.speechThreshold ? this.quiet + samples.length : 0;
		const paused =
			this.buffered >= this.samples(this.opts.minPartMs) &&
			this.quiet >= this.samples(this.opts.pauseMs);
		if (paused || this.buffered >= this.samples(this.opts.maxPartMs)) this.flush(false);
	}

	end() {
		if (!this.speaking) return;
		this.speaking = false;
		if (this.buffered) this.flush(true);
		else if (this.sent) this.emit(new Float32Array(0), true);
		this.sent = 0;
	}

	cancel() {
		if (!this.speaking) return;
		this.speaking = false;
		this.frames = [];
		this.buffered = 0;
		this.quiet = 0;
		if (this.sent) this.emit(new Float32Array(0), true);
		this.sent = 0;
	}

	private flush(last: boolean) {
		const out = new Float32Array(this.buffered);
		let at = 0;
		for (const f of this.frames) {
			out.set(f, at);
			at += f.length;
		}
		this.frames = [];
		this.buffered = 0;
		this.quiet = 0;
		this.sent++;
		this.emit(out, last);
	}

	private samples(ms: number): number {
		return Math.round((ms * this.opts.sampleRate) / 1000);
	}
}
