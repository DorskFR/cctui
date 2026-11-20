/** The dictation mic's gesture state: a tap starts recording and a second
 *  tap stops it; a press held past `HOLD_MS` while speaking records until
 *  release. */

/** Long enough that a firm click on a trackpad is still a tap. */
export const HOLD_MS = 800;
/** A recording stopped sooner than this holds nothing worth transcribing. */
export const MIN_CLIP_MS = 300;
export const SILENT_CLIP_MS = 1500;

export type TapHoldAction = 'start' | 'stop' | 'cancel' | 'none';

export class TapHold {
	recording = false;
	private startedAt = 0;
	private pressAt: number | null = null;
	private pressStarted = false;
	private captured = false;
	private readonly heard: () => boolean;

	constructor(heard: () => boolean = () => true) {
		this.heard = heard;
	}

	down(now: number): TapHoldAction {
		if (this.recording) return this.finish(now);
		this.recording = true;
		this.startedAt = now;
		this.pressAt = now;
		this.pressStarted = true;
		return 'start';
	}

	capture() {
		if (this.pressStarted) this.captured = true;
	}

	up(now: number): TapHoldAction {
		const pressAt = this.pressAt;
		this.pressAt = null;
		this.captured = false;
		if (pressAt === null || !this.pressStarted) return 'none';
		this.pressStarted = false;
		return now - pressAt >= HOLD_MS && this.heard() ? this.finish(now) : 'none';
	}

	leave(now: number): TapHoldAction {
		return this.captured ? 'none' : this.up(now);
	}

	/** Keyboard or the stop control: start, or stop what is recording. */
	toggle(now: number): TapHoldAction {
		return this.recording ? this.finish(now) : this.down(now);
	}

	reset() {
		this.recording = false;
		this.pressAt = null;
		this.pressStarted = false;
		this.captured = false;
	}

	private finish(now: number): TapHoldAction {
		const elapsed = now - this.startedAt;
		this.reset();
		const silent = elapsed < SILENT_CLIP_MS && !this.heard();
		return elapsed < MIN_CLIP_MS || silent ? 'cancel' : 'stop';
	}
}
