import { toasts as kit } from '@dorsk/tsumikit';

export { type Toast, type ToastAction, type ToastOptions } from '@dorsk/tsumikit';

/**
 * Two affordances on one bubble can fail from the same click, and a stack of
 * identical error toasts reads as two separate faults. Repeats of a message
 * already on screen within this window are dropped.
 */
const DEDUPE_MS = 2_000;

const lastShown = new Map<string, { at: number; id: unknown }>();

const rawError = kit.error.bind(kit) as (...args: unknown[]) => unknown;

export const toasts = {
	get items() {
		return kit.items;
	},
	ok: kit.ok.bind(kit),
	info: kit.info.bind(kit),
	dismiss: kit.dismiss.bind(kit),
	act: kit.act.bind(kit),
	error: ((message: unknown, ...rest: unknown[]) => {
		const now = Date.now();
		const key = String(message);
		for (const [k, seen] of lastShown) if (now - seen.at >= DEDUPE_MS) lastShown.delete(k);
		const previous = lastShown.get(key);
		if (previous) return previous.id;
		const id = rawError(message, ...rest);
		lastShown.set(key, { at: now, id });
		return id;
	}) as typeof kit.error
};
