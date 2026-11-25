import { saveErrorMessage } from '#lib/saveError.ts';

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

	async run<T>(write: () => Promise<T>): Promise<T | undefined> {
		this.#inflight++;
		this.#set('pending');
		try {
			const out = await write();
			if (--this.#inflight === 0) {
				this.savedAt = Date.now();
				this.error = null;
				this.#set('saved');
			}
			return out;
		} catch (e) {
			this.#inflight--;
			this.error = saveErrorMessage(e);
			this.#set('error');
			return undefined;
		}
	}

	#set(status: SaveStatus) {
		this.status = status;
		this.at = Date.now();
	}
}

export const serverSave = new ServerSave();

/** The save state the indicator shows: anything in flight, else the latest outcome. */
export function latestSave(user: Omit<SaveState, 'at'>, server: SaveState): Omit<SaveState, 'at'> {
	if (user.status === 'pending') return user;
	if (server.status === 'pending') return server;
	if (server.status === 'idle') return user;
	if (user.status === 'idle') return server;
	return server.at >= (user.savedAt ?? 0) ? server : user;
}
