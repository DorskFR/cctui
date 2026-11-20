/** Keeps a spoken reply from talking over the user. While they speak the gate
 *  is shut and reply audio waits; it opens `delayMs` after they stop. A reply
 *  that arrived while shut was talked over: once the user's own message goes
 *  out, `userMessage` says to drop it rather than read it over the next
 *  exchange. */
export interface ReplyGateOptions {
	delayMs: () => number;
	onChange: (open: boolean) => void;
}

export class ReplyGate {
	private talking = false;
	private timer: ReturnType<typeof setTimeout> | null = null;
	private overlapped = false;
	private readonly opts: ReplyGateOptions;

	constructor(opts: ReplyGateOptions) {
		this.opts = opts;
	}

	get open(): boolean {
		return !this.talking && this.timer === null;
	}

	speechStart() {
		const was = this.open;
		this.clearTimer();
		this.talking = true;
		if (was) this.opts.onChange(false);
	}

	speechEnd() {
		if (!this.talking) return;
		this.talking = false;
		const ms = this.opts.delayMs();
		if (!(ms > 0)) {
			this.opts.onChange(true);
			return;
		}
		this.timer = setTimeout(() => {
			this.timer = null;
			this.opts.onChange(true);
		}, ms);
	}

	/** A reply line arrived; `true` when it may play right away. */
	reply(): boolean {
		if (this.open) return true;
		this.overlapped = true;
		return false;
	}

	drained() {
		if (this.open) this.overlapped = false;
	}

	userMessage(): boolean {
		const drop = this.overlapped;
		this.overlapped = false;
		return drop;
	}

	reset() {
		const was = this.open;
		this.clearTimer();
		this.talking = false;
		this.overlapped = false;
		if (!was) this.opts.onChange(true);
	}

	private clearTimer() {
		if (this.timer) clearTimeout(this.timer);
		this.timer = null;
	}
}
