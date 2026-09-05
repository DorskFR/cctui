// Touch-only left swipe of a row: arms once the gesture is clearly horizontal
// (vertical stays a native scroll), commits past ~40% of the row width,
// springs back otherwise. `consumeClick()` lets the host drop the click that
// trails a drag.
export class SwipeGesture {
	x = $state(0);
	active = $state(false);
	#armed = false;
	#didSwipe = false;
	#sx = 0;
	#sy = 0;
	#cardW = $state(0);
	#enabled: () => boolean;
	#oncommit: () => void;

	constructor(enabled: () => boolean, oncommit: () => void) {
		this.#enabled = enabled;
		this.#oncommit = oncommit;
	}

	get threshold(): number {
		return this.#cardW ? this.#cardW * 0.4 : Number.POSITIVE_INFINITY;
	}
	get progress(): number {
		return Math.min(1, -this.x / this.threshold);
	}

	consumeClick(): boolean {
		if (!this.#didSwipe) return false;
		this.#didSwipe = false;
		return true;
	}

	start = (e: PointerEvent) => {
		if (!this.#enabled() || e.pointerType !== 'touch') return;
		this.#sx = e.clientX;
		this.#sy = e.clientY;
		this.#cardW = (e.currentTarget as HTMLElement).offsetWidth;
		this.#armed = false;
		this.#didSwipe = false;
	};

	move = (e: PointerEvent) => {
		if (!this.#enabled() || e.pointerType !== 'touch' || !this.#cardW) return;
		const dx = e.clientX - this.#sx;
		const dy = e.clientY - this.#sy;
		if (!this.#armed) {
			if (Math.abs(dx) < 12) return;
			if (Math.abs(dx) <= Math.abs(dy) * 1.5) {
				this.#cardW = 0;
				return;
			}
			this.#armed = true;
			this.active = true;
			try {
				(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
			} catch {
				/* capture unsupported — move events still arrive */
			}
		}
		this.x = Math.min(0, dx);
	};

	end = () => {
		if (!this.active) {
			this.#cardW = 0;
			this.#armed = false;
			return;
		}
		const commit = -this.x >= this.threshold;
		this.active = false;
		this.#armed = false;
		this.#didSwipe = true;
		if (commit) {
			if (typeof navigator !== 'undefined' && navigator.vibrate) navigator.vibrate(20);
			this.x = -this.#cardW;
			this.#oncommit();
		} else {
			this.x = 0;
		}
		this.#cardW = 0;
	};
}
