import { createSubscriber } from 'svelte/reactivity';

const TICK_MS = 1_000;
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function listen(fn: () => void): () => void {
	listeners.add(fn);
	timer ??= setInterval(() => {
		for (const f of listeners) f();
	}, TICK_MS);
	return () => {
		listeners.delete(fn);
		if (!listeners.size && timer !== null) {
			clearInterval(timer);
			timer = null;
		}
	};
}

class Clock {
	#at = Date.now();
	#live = false;
	#subscribe: () => void;

	constructor(periodMs: number) {
		this.#subscribe = createSubscriber((update) => {
			this.#live = true;
			this.#at = Date.now();
			const off = listen(() => {
				const t = Date.now();
				if (Math.floor(t / periodMs) === Math.floor(this.#at / periodMs)) return;
				this.#at = t;
				update();
			});
			return () => {
				this.#live = false;
				off();
			};
		});
	}

	get now(): number {
		this.#subscribe();
		return this.#live ? this.#at : Date.now();
	}
}

const clocks = new Map<number, Clock>();

/** Wall-clock ms, reactive: dependents re-run once per `periodMs`. Every
 * cadence shares one interval, which only runs while something reads it. */
export function now(periodMs = TICK_MS): number {
	let c = clocks.get(periodMs);
	if (!c) {
		c = new Clock(periodMs);
		clocks.set(periodMs, c);
	}
	return c.now;
}

export function clockRunning(): boolean {
	return timer !== null;
}
