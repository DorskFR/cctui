import { saveErrorMessage } from '#lib/saveError.ts';

/** Pause after the last edit before an auto-saved setting is written. */
export const SAVE_DEBOUNCE_MS = 600;

export type SaveStatus = 'idle' | 'pending' | 'saved' | 'error';

export interface SaveState {
	status: SaveStatus;
	savedAt: number | null;
	error: string | null;
	/** When `status` last changed. */
	at: number;
}

/** Tracks auto-saved server-side settings so the page indicator covers them too. */
class ServerSave implements SaveState {
	status = $state<SaveStatus>('idle');
	savedAt = $state<number | null>(null);
	error = $state<string | null>(null);
	at = $state(0);
	#inflight = 0;
	/** Set once any write of the current overlapping batch fails. */
	#failed: { message: string | null } | null = null;

	async run<T>(write: () => Promise<T>): Promise<T | undefined> {
		if (this.#inflight++ === 0) this.#failed = null;
		this.#set('pending');
		let out: T | undefined;
		try {
			out = await write();
		} catch (e) {
			this.#failed = { message: saveErrorMessage(e) };
		}
		if (--this.#inflight === 0) {
			if (this.#failed) {
				this.error = this.#failed.message;
				this.#set('error');
			} else {
				this.savedAt = Date.now();
				this.error = null;
				this.#set('saved');
			}
		}
		return out;
	}

	#set(status: SaveStatus) {
		this.status = status;
		this.at = Date.now();
	}
}

export const serverSave = new ServerSave();

/** The save state the indicator shows: anything in flight, else the latest outcome. */
export function latestSave(user: SaveState, server: SaveState): SaveState {
	if (user.status === 'pending') return user;
	if (server.status === 'pending') return server;
	if (server.status === 'idle') return user;
	if (user.status === 'idle') return server;
	return server.at >= user.at ? server : user;
}

/**
 * Writes `next()` once it has stopped changing for `SAVE_DEBOUNCE_MS`.
 * `next` must read the draft values themselves (not only a dirty flag) so
 * every edit restarts the wait; `undefined` means nothing to save. A write
 * still waiting when the component unmounts is sent right away.
 */
export function autoSave<T>(next: () => T | undefined, write: (value: T) => unknown): void {
	let timer: ReturnType<typeof setTimeout> | undefined;
	let pending: (() => void) | null = null;
	$effect(() => {
		const value = next();
		clearTimeout(timer);
		if (value === undefined) {
			pending = null;
			return;
		}
		const send = () => {
			pending = null;
			void write(value);
		};
		pending = send;
		timer = setTimeout(send, SAVE_DEBOUNCE_MS);
	});
	$effect(() => () => {
		clearTimeout(timer);
		pending?.();
	});
}
