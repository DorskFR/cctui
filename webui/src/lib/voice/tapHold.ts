/** The dictation mic's gesture state: a tap starts recording and a second
 *  tap stops it; a press held past `HOLD_MS` records until release. */

export const HOLD_MS = 300;
/** A recording stopped sooner than this holds nothing worth transcribing. */
export const MIN_CLIP_MS = 300;

export type TapHoldAction = 'start' | 'stop' | 'cancel' | 'none';

export class TapHold {
	recording = false;
	private startedAt = 0;
	private pressAt: number | null = null;
	private pressStarted = false;

	down(now: number): TapHoldAction {
		if (this.recording) return this.finish(now);
		this.recording = true;
		this.startedAt = now;
		this.pressAt = now;
		this.pressStarted = true;
		return 'start';
	}

	/** A release, or the pointer leaving the control mid-press. */
	up(now: number): TapHoldAction {
		const pressAt = this.pressAt;
		this.pressAt = null;
		if (pressAt === null || !this.pressStarted) return 'none';
		this.pressStarted = false;
		return now - pressAt >= HOLD_MS ? this.finish(now) : 'none';
	}

	/** Keyboard or the stop control: start, or stop what is recording. */
	toggle(now: number): TapHoldAction {
		return this.recording ? this.finish(now) : this.down(now);
	}

	reset() {
		this.recording = false;
		this.pressAt = null;
		this.pressStarted = false;
	}

	private finish(now: number): TapHoldAction {
		const elapsed = now - this.startedAt;
		this.reset();
		return elapsed < MIN_CLIP_MS ? 'cancel' : 'stop';
	}
}
